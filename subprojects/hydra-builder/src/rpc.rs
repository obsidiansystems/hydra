use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, StreamBody};
use hyper::body::Frame;
use hyper_util::rt::TokioIo;
use protocol::{BuilderMessage, RunnerMessage, frame};

pub const PROTO_API_VERSION: &str = env!("CARGO_PKG_VERSION");

type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

fn postcard_body(data: &[u8]) -> BoxBody {
    Full::new(Bytes::copy_from_slice(data))
        .map_err(|never| match never {})
        .boxed()
}

fn empty_body() -> BoxBody {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed()
}

#[derive(Debug, Clone)]
pub struct RpcClient {
    sender: hyper::client::conn::http2::SendRequest<BoxBody>,
    auth_header: Option<String>,
}

impl RpcClient {
    async fn request<Req: serde::Serialize, Resp: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> anyhow::Result<Resp> {
        let encoded = postcard::to_allocvec(body).context("encode request")?;
        let mut builder = hyper::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/x-postcard");
        if let Some(ref token) = self.auth_header {
            builder = builder.header("authorization", token.as_str());
        }
        let req = builder.body(postcard_body(&encoded))?;

        let resp = self.sender.clone().send_request(req).await?;
        let status = resp.status();
        let body_bytes = resp.into_body().collect().await?.to_bytes();

        if !status.is_success() {
            if let Ok(err) = postcard::from_bytes::<protocol::RpcError>(&body_bytes) {
                anyhow::bail!("RPC error {}: {}", err.code, err.message);
            }
            anyhow::bail!("RPC error: status {status}");
        }

        postcard::from_bytes(&body_bytes).context("decode response")
    }

    async fn post_empty<Req: serde::Serialize>(
        &self,
        path: &str,
        body: &Req,
    ) -> anyhow::Result<()> {
        let encoded = postcard::to_allocvec(body).context("encode request")?;
        let mut builder = hyper::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/x-postcard");
        if let Some(ref token) = self.auth_header {
            builder = builder.header("authorization", token.as_str());
        }
        let req = builder.body(postcard_body(&encoded))?;

        let resp = self.sender.clone().send_request(req).await?;
        let status = resp.status();
        if !status.is_success() {
            let body_bytes = resp.into_body().collect().await?.to_bytes();
            if let Ok(err) = postcard::from_bytes::<protocol::RpcError>(&body_bytes) {
                anyhow::bail!("RPC error {}: {}", err.code, err.message);
            }
            anyhow::bail!("RPC error: status {status}");
        }
        Ok(())
    }

    pub async fn check_version(
        &self,
        req: &protocol::VersionCheckRequest,
    ) -> anyhow::Result<protocol::VersionCheckResponse> {
        self.request("/rpc/check-version", req).await
    }

    pub async fn build_step_update(&self, update: &protocol::StepUpdate) -> anyhow::Result<()> {
        self.post_empty("/rpc/step-update", update).await
    }

    pub async fn fetch_drv_requisites(
        &self,
        req: &protocol::FetchRequisitesRequest,
    ) -> anyhow::Result<protocol::DrvRequisitesMessage> {
        self.request("/rpc/fetch-requisites", req).await
    }

    pub async fn has_path(&self, path: &str) -> anyhow::Result<protocol::HasPathResponse> {
        self.request("/rpc/has-path", &path.to_owned()).await
    }

    pub async fn complete_build(
        &self,
        info: &protocol::BuildResultInfo,
    ) -> anyhow::Result<()> {
        self.post_empty("/rpc/complete-build", info).await
    }

    pub async fn request_presigned_urls(
        &self,
        build_id: &str,
        machine_id: &str,
        store_paths: Vec<(nix_utils::StorePath, String, Vec<String>)>,
    ) -> anyhow::Result<Vec<protocol::PresignedNarResponse>> {
        let request = store_paths
            .into_iter()
            .map(|(path, nar_hash, build_ids)| protocol::PresignedNarRequest {
                store_path: path.to_string().to_owned(),
                nar_hash,
                debug_info_build_ids: build_ids,
            })
            .collect::<Vec<_>>();

        let resp: protocol::PresignedUrlResponse = self
            .request(
                "/rpc/presigned-url",
                &protocol::PresignedUrlRequest {
                    build_id: build_id.to_owned(),
                    machine_id: machine_id.to_owned(),
                    request,
                },
            )
            .await
            .context("Failed to request presigned URLs")?;
        Ok(resp.inner)
    }

    pub async fn notify_presigned_upload_complete(
        &self,
        msg: &protocol::PresignedUploadComplete,
    ) -> anyhow::Result<()> {
        self.post_empty("/rpc/presigned-complete", msg).await
    }

    /// Send a streaming POST of length-prefixed NarData frames to /rpc/build-log.
    pub async fn build_log_stream(
        &self,
        rx: tokio::sync::mpsc::UnboundedReceiver<protocol::LogChunk>,
    ) -> anyhow::Result<()> {
        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        let frame_stream = tokio_stream::StreamExt::map(stream, |chunk| {
            match frame::encode(&chunk) {
                Ok(data) => Ok(Frame::data(Bytes::from(data))),
                Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
            }
        });
        let body: BoxBody = StreamBody::new(frame_stream)
            .map_err(|e| {
                // This is only used as the error type for BoxBody; hyper::Error doesn't
                // have a public constructor so we use a workaround via the io error path.
                let _ = e;
                unreachable!("frame encoding should not fail")
            })
            .boxed();

        let mut builder = hyper::Request::builder()
            .method("POST")
            .uri("/rpc/build-log")
            .header("content-type", "application/x-postcard-stream");
        if let Some(ref token) = self.auth_header {
            builder = builder.header("authorization", token.as_str());
        }
        let req = builder.body(body)?;

        let resp = self.sender.clone().send_request(req).await?;
        if !resp.status().is_success() {
            anyhow::bail!("build_log RPC failed: status {}", resp.status());
        }
        Ok(())
    }

    /// Send a streaming POST of length-prefixed NarData frames to /rpc/build-result.
    pub async fn build_result_stream(
        &self,
        rx: tokio::sync::mpsc::UnboundedReceiver<protocol::NarData>,
    ) -> anyhow::Result<()> {
        let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
        let frame_stream = tokio_stream::StreamExt::map(stream, |chunk| {
            match frame::encode(&chunk) {
                Ok(data) => Ok::<_, std::convert::Infallible>(Frame::data(Bytes::from(data))),
                Err(_) => unreachable!("frame encoding should not fail"),
            }
        });
        let body: BoxBody = StreamBody::new(frame_stream)
            .map_err(|never| match never {})
            .boxed();

        let mut builder = hyper::Request::builder()
            .method("POST")
            .uri("/rpc/build-result")
            .header("content-type", "application/x-postcard-stream");
        if let Some(ref token) = self.auth_header {
            builder = builder.header("authorization", token.as_str());
        }
        let req = builder.body(body)?;

        let resp = self.sender.clone().send_request(req).await?;
        if !resp.status().is_success() {
            anyhow::bail!("build_result RPC failed: status {}", resp.status());
        }
        Ok(())
    }

    /// Stream files from the queue-runner via POST /rpc/stream-files.
    /// Returns a receiver of NarData chunks.
    pub async fn stream_files(
        &self,
        paths: Vec<String>,
    ) -> anyhow::Result<tokio::sync::mpsc::UnboundedReceiver<Result<Bytes, std::io::Error>>> {
        let encoded = postcard::to_allocvec(&paths).context("encode stream-files request")?;
        let mut builder = hyper::Request::builder()
            .method("POST")
            .uri("/rpc/stream-files")
            .header("content-type", "application/x-postcard");
        if let Some(ref token) = self.auth_header {
            builder = builder.header("authorization", token.as_str());
        }
        let req = builder.body(postcard_body(&encoded))?;

        let resp = self.sender.clone().send_request(req).await?;
        if !resp.status().is_success() {
            anyhow::bail!("stream_files RPC failed: status {}", resp.status());
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut body = resp.into_body();
        let mut reader = protocol::FrameReader::new();

        tokio::spawn(async move {
            loop {
                match body.frame().await {
                    Some(Ok(frame)) => {
                        if let Some(data) = frame.data_ref() {
                            reader.extend(data);
                        }
                        while let Some(payload) = reader.next_frame() {
                            match postcard::from_bytes::<protocol::NarData>(&payload) {
                                Ok(chunk) => {
                                    if tx.send(Ok(Bytes::from(chunk.chunk))).is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx.send(Err(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        e,
                                    )));
                                    return;
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx.send(Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            e,
                        )));
                        return;
                    }
                    None => return,
                }
            }
        });

        Ok(rx)
    }
}

#[tracing::instrument(err)]
pub async fn init_client(cli: &crate::config::Cli) -> anyhow::Result<RpcClient> {
    if !cli.mtls_configured_correctly() {
        tracing::error!(
            "mtls configured improperly, please pass all options: \
            server_root_ca_cert_path, client_cert_path, client_key_path and domain_name!"
        );
        return Err(anyhow::anyhow!("Configuration issue"));
    }

    tracing::info!("connecting to {}", cli.gateway_endpoint);

    let sender = if let Some(path) = cli.gateway_endpoint.strip_prefix("unix://") {
        let path = path.to_owned();
        let stream = tokio::net::UnixStream::connect(&path).await?;
        let io = TokioIo::new(stream);
        let (sender, conn) = hyper::client::conn::http2::handshake(
            hyper_util::rt::TokioExecutor::new(),
            io,
        )
        .await?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::error!("HTTP/2 connection error: {e}");
            }
        });
        sender
    } else {
        let uri: url::Url = cli
            .gateway_endpoint
            .parse()
            .context("Failed to parse gateway_endpoint")?;
        let host = uri
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("No host in gateway_endpoint"))?;
        let port = uri.port().unwrap_or(if uri.scheme() == "https" { 443 } else { 80 });
        let addr = format!("{host}:{port}");

        if uri.scheme() == "https" || cli.mtls_enabled() {
            let mut root_store = tokio_rustls::rustls::RootCertStore::empty();

            let tls_config = if cli.mtls_enabled() {
                let (ca_pem, client_cert_pem, client_key_pem, _domain) = cli
                    .get_mtls()
                    .await
                    .context("Failed to load mTLS certificates")?;
                let ca_certs = rustls_pemfile::certs(&mut ca_pem.as_bytes())
                    .collect::<Result<Vec<_>, _>>()?;
                for cert in ca_certs {
                    root_store.add(cert)?;
                }
                let client_certs = rustls_pemfile::certs(&mut client_cert_pem.as_bytes())
                    .collect::<Result<Vec<_>, _>>()?;
                let client_key = rustls_pemfile::private_key(&mut client_key_pem.as_bytes())?
                    .ok_or_else(|| anyhow::anyhow!("no private key found in client key PEM"))?;
                tokio_rustls::rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_client_auth_cert(client_certs, client_key)?
            } else {
                root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                tokio_rustls::rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            };

            let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config));
            let tcp = tokio::net::TcpStream::connect(&addr).await?;
            let domain = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_owned())?;
            let tls_stream = connector.connect(domain, tcp).await?;
            let io = TokioIo::new(tls_stream);
            let (sender, conn) = hyper::client::conn::http2::handshake(
                hyper_util::rt::TokioExecutor::new(),
                io,
            )
            .await?;
            tokio::spawn(async move {
                if let Err(e) = conn.await {
                    tracing::error!("HTTP/2 connection error: {e}");
                }
            });
            sender
        } else {
            let tcp = tokio::net::TcpStream::connect(&addr).await?;
            let io = TokioIo::new(tcp);
            let (sender, conn) = hyper::client::conn::http2::handshake(
                hyper_util::rt::TokioExecutor::new(),
                io,
            )
            .await?;
            tokio::spawn(async move {
                if let Err(e) = conn.await {
                    tracing::error!("HTTP/2 connection error: {e}");
                }
            });
            sender
        }
    };

    let auth_header = if let Some(t) = cli.get_authorization_token().await? {
        Some(format!("Bearer {t}"))
    } else {
        None
    };

    Ok(RpcClient {
        sender,
        auth_header,
    })
}

#[tracing::instrument(skip(state), err)]
async fn handle_request(
    state: Arc<crate::state::State>,
    request: RunnerMessage,
) -> anyhow::Result<()> {
    match request {
        RunnerMessage::Join(m) => {
            state
                .max_concurrent_downloads
                .store(m.max_concurrent_downloads, Ordering::Relaxed);
        }
        RunnerMessage::ConfigUpdate(m) => {
            state
                .max_concurrent_downloads
                .store(m.max_concurrent_downloads, Ordering::Relaxed);
        }
        RunnerMessage::Ping(_) => (),
        RunnerMessage::Build(m) => {
            state.schedule_build(m)?;
        }
        RunnerMessage::Abort(m) => {
            state.abort_build(&m)?;
        }
    }
    Ok(())
}

#[tracing::instrument(skip(state), err)]
async fn check_version_compatibility(state: Arc<crate::state::State>) -> anyhow::Result<()> {
    let response = state
        .client
        .check_version(&protocol::VersionCheckRequest {
            version: PROTO_API_VERSION.to_string(),
            machine_id: state.id.to_string(),
            hostname: state.hostname.clone(),
        })
        .await?;

    if !response.compatible {
        return Err(anyhow::anyhow!(
            "API version mismatch: client has {}, server has {}",
            PROTO_API_VERSION,
            response.server_version,
        ));
    }

    tracing::info!(
        "Version check passed: client={}, server={}",
        PROTO_API_VERSION,
        response.server_version
    );
    Ok(())
}

#[tracing::instrument(skip(state), err)]
pub async fn start_bidirectional_stream(state: Arc<crate::state::State>) -> anyhow::Result<()> {
    check_version_compatibility(state.clone()).await?;

    let join_msg = state.get_join_message().await?;

    // Start the tunnel: POST join + pings to /rpc/tunnel/send
    let join_frame = frame::encode(&BuilderMessage::Join(join_msg))?;
    let (ping_tx, ping_rx) = tokio::sync::mpsc::unbounded_channel::<Result<Frame<Bytes>, std::convert::Infallible>>();

    // Send join as first frame
    let _ = ping_tx.send(Ok(Frame::data(Bytes::from(join_frame))));

    // Spawn ping sender
    let state2 = state.clone();
    let ping_tx2 = ping_tx.clone();
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(state2.config.ping_interval));
        loop {
            interval.tick().await;
            let ping = match state2.get_ping_message() {
                Ok(v) => BuilderMessage::Ping(v),
                Err(e) => {
                    tracing::error!("Failed to construct ping message: {e}");
                    continue;
                }
            };
            tracing::debug!("sending ping: {ping:?}");
            match frame::encode(&ping) {
                Ok(data) => {
                    if ping_tx2.send(Ok(Frame::data(Bytes::from(data)))).is_err() {
                        break;
                    }
                }
                Err(e) => tracing::error!("failed to encode ping: {e}"),
            }
        }
    });

    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(ping_rx);
    let body: BoxBody = StreamBody::new(stream)
        .map_err(|never| match never {})
        .boxed();

    let mut builder = hyper::Request::builder()
        .method("POST")
        .uri("/rpc/tunnel/send")
        .header("content-type", "application/x-postcard-stream");
    if let Some(ref token) = state.client.auth_header {
        builder = builder.header("authorization", token.as_str());
    }
    let send_req = builder.body(body)?;

    // Send the tunnel/send request (fire and forget, pings continue in background)
    let mut sender = state.client.sender.clone();
    let send_resp = sender.send_request(send_req).await?;
    if !send_resp.status().is_success() {
        anyhow::bail!("tunnel/send failed: status {}", send_resp.status());
    }

    // Now GET /rpc/tunnel/recv to receive runner messages
    let id_string = state.id.to_string();
    let machine_id_str = urlencoding::encode(&id_string);
    let recv_uri = format!("/rpc/tunnel/recv?machine_id={machine_id_str}");
    let mut builder = hyper::Request::builder().method("GET").uri(&recv_uri);
    if let Some(ref token) = state.client.auth_header {
        builder = builder.header("authorization", token.as_str());
    }
    let recv_req = builder.body(empty_body())?;

    let recv_resp = sender.send_request(recv_req).await?;
    if !recv_resp.status().is_success() {
        anyhow::bail!("tunnel/recv failed: status {}", recv_resp.status());
    }

    let mut body = recv_resp.into_body();
    let mut reader = protocol::FrameReader::new();
    let mut consecutive_failure_count = 0;

    loop {
        match body.frame().await {
            Some(Ok(frame)) => {
                if let Some(data) = frame.data_ref() {
                    reader.extend(data);
                }
                while let Some(payload) = reader.next_frame() {
                    match postcard::from_bytes::<RunnerMessage>(&payload) {
                        Ok(msg) => {
                            consecutive_failure_count = 0;
                            if let Err(err) = handle_request(state.clone(), msg).await {
                                tracing::error!("Failed to correctly handle request: {err}");
                            }
                        }
                        Err(e) => {
                            consecutive_failure_count += 1;
                            tracing::error!("failed to decode runner message: {e}");
                            if consecutive_failure_count == 10 {
                                return Err(anyhow::anyhow!(
                                    "Failed to decode {consecutive_failure_count} messages. \
                                    Terminating the application."
                                ));
                            }
                        }
                    }
                }
            }
            Some(Err(e)) => {
                consecutive_failure_count += 1;
                tracing::error!("stream message delivery failed: {e}");
                if consecutive_failure_count == 10 {
                    return Err(anyhow::anyhow!(
                        "Failed to communicate {consecutive_failure_count} times over the channel. \
                        Terminating the application."
                    ));
                }
            }
            None => break,
        }
    }

    Ok(())
}
