use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, StreamBody};
use hyper::body::Frame;
use tokio::{io::AsyncWriteExt as _, sync::mpsc};
use tracing::Instrument as _;

use crate::{
    config::BindSocket,
    state::{Machine, MachineMessage, State},
};
use nix_utils::BaseStore as _;
use protocol::{frame, BuilderMessage, RpcError};

pub const PROTO_API_VERSION: &str = env!("CARGO_PKG_VERSION");

type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

fn empty_body() -> BoxBody {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed()
}

fn postcard_response<T: serde::Serialize>(msg: &T) -> hyper::Response<BoxBody> {
    match postcard::to_allocvec(msg) {
        Ok(body) => hyper::Response::builder()
            .status(200)
            .header("content-type", "application/x-postcard")
            .body(
                Full::new(Bytes::from(body))
                    .map_err(|never| match never {})
                    .boxed(),
            )
            .expect("response builder should not fail"),
        Err(e) => error_response(500, &format!("serialization error: {e}")),
    }
}

fn error_response(code: u16, message: &str) -> hyper::Response<BoxBody> {
    let err = RpcError {
        code,
        message: message.to_owned(),
    };
    let body = postcard::to_allocvec(&err).unwrap_or_default();
    hyper::Response::builder()
        .status(code)
        .header("content-type", "application/x-postcard")
        .body(
            Full::new(Bytes::from(body))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("response builder should not fail")
}

fn check_auth(
    req: &hyper::Request<hyper::body::Incoming>,
    config: &crate::config::App,
) -> Result<(), hyper::Response<BoxBody>> {
    if config.has_token_list() {
        match req.headers().get("authorization") {
            Some(v) => {
                let s = v.to_str().map_err(|_| error_response(401, "No valid auth token"))?;
                let token = s
                    .strip_prefix("Bearer ")
                    .ok_or_else(|| error_response(401, "No valid auth token"))?;
                if config.check_if_contains_token(token) {
                    Ok(())
                } else {
                    Err(error_response(401, "No valid auth token"))
                }
            }
            None => Err(error_response(401, "No valid auth token")),
        }
    } else {
        Ok(())
    }
}

async fn read_body(req: hyper::Request<hyper::body::Incoming>) -> Result<Bytes, hyper::Response<BoxBody>> {
    req.into_body()
        .collect()
        .await
        .map(|c| c.to_bytes())
        .map_err(|e| error_response(500, &format!("failed to read body: {e}")))
}

fn decode_body<'de, T: serde::Deserialize<'de>>(body: &'de [u8]) -> Result<T, hyper::Response<BoxBody>> {
    postcard::from_bytes(body).map_err(|e| error_response(400, &format!("decode error: {e}")))
}

// there is no reason to make this configurable, it only exists so we ensure the channel is not
// closed. we dont use this to write any actual information.
const BACKWARDS_PING_INTERVAL: u64 = 30;

fn handle_ping(state: &Arc<State>, msg: &protocol::PingMessage) {
    let Ok(machine_id) = uuid::Uuid::parse_str(&msg.machine_id) else {
        return;
    };
    if let Some(m) = state.machines.get_machine_by_id(machine_id) {
        m.stats.store_ping(msg);
    }
}

#[allow(missing_debug_implementations)]
#[derive(Clone)]
pub struct Server {
    state: Arc<State>,
}

impl Server {
    #[tracing::instrument(skip(state), err)]
    pub async fn run(addr: BindSocket, state: Arc<State>) -> anyhow::Result<()> {
        let server = Arc::new(Self {
            state: state.clone(),
        });

        match addr {
            BindSocket::Tcp(s) => {
                let listener = tokio::net::TcpListener::bind(&s).await?;
                loop {
                    let (stream, _) = listener.accept().await?;
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let server = server.clone();
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |req| {
                            let server = server.clone();
                            async move { server.route(req).await }
                        });
                        if let Err(e) = hyper::server::conn::http2::Builder::new(
                            hyper_util::rt::TokioExecutor::new(),
                        )
                        .serve_connection(io, service)
                        .await
                        {
                            tracing::error!("connection error: {e}");
                        }
                    });
                }
            }
            BindSocket::Unix(p) => {
                let listener = tokio::net::UnixListener::bind(p)?;
                loop {
                    let (stream, _) = listener.accept().await?;
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let server = server.clone();
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |req| {
                            let server = server.clone();
                            async move { server.route(req).await }
                        });
                        if let Err(e) = hyper::server::conn::http2::Builder::new(
                            hyper_util::rt::TokioExecutor::new(),
                        )
                        .serve_connection(io, service)
                        .await
                        {
                            tracing::error!("connection error: {e}");
                        }
                    });
                }
            }
            BindSocket::ListenFd => {
                let listener = listenfd::ListenFd::from_env()
                    .take_unix_listener(0)?
                    .ok_or_else(|| anyhow::anyhow!("No listenfd found in env"))?;
                listener.set_nonblocking(true)?;
                let listener = tokio::net::UnixListener::from_std(listener)?;
                loop {
                    let (stream, _) = listener.accept().await?;
                    let io = hyper_util::rt::TokioIo::new(stream);
                    let server = server.clone();
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |req| {
                            let server = server.clone();
                            async move { server.route(req).await }
                        });
                        if let Err(e) = hyper::server::conn::http2::Builder::new(
                            hyper_util::rt::TokioExecutor::new(),
                        )
                        .serve_connection(io, service)
                        .await
                        {
                            tracing::error!("connection error: {e}");
                        }
                    });
                }
            }
        }
    }

    async fn route(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, std::convert::Infallible> {
        let resp = match (req.method(), req.uri().path()) {
            (&hyper::Method::GET, "/health") => Ok(hyper::Response::builder()
                .status(200)
                .body(empty_body())
                .expect("response builder should not fail")),
            _ => {
                if let Err(resp) = check_auth(&req, &self.state.config) {
                    return Ok(resp);
                }
                match (req.method(), req.uri().path()) {
                    (&hyper::Method::POST, "/rpc/check-version") => self.handle_check_version(req).await,
                    (&hyper::Method::POST, "/rpc/tunnel/send") => self.handle_tunnel_send(req).await,
                    (&hyper::Method::GET, "/rpc/tunnel/recv") => self.handle_tunnel_recv(req).await,
                    (&hyper::Method::POST, "/rpc/build-log") => self.handle_build_log(req).await,
                    (&hyper::Method::POST, "/rpc/build-result") => self.handle_build_result(req).await,
                    (&hyper::Method::POST, "/rpc/step-update") => self.handle_step_update(req).await,
                    (&hyper::Method::POST, "/rpc/complete-build") => self.handle_complete_build(req).await,
                    (&hyper::Method::POST, "/rpc/fetch-requisites") => self.handle_fetch_requisites(req).await,
                    (&hyper::Method::POST, "/rpc/has-path") => self.handle_has_path(req).await,
                    (&hyper::Method::GET, "/rpc/stream-file") => self.handle_stream_file(req).await,
                    (&hyper::Method::POST, "/rpc/stream-files") => self.handle_stream_files(req).await,
                    (&hyper::Method::POST, "/rpc/presigned-url") => self.handle_presigned_url(req).await,
                    (&hyper::Method::POST, "/rpc/presigned-complete") => self.handle_presigned_complete(req).await,
                    _ => Ok(error_response(404, "not found")),
                }
            }
        };

        Ok(resp.unwrap_or_else(|r| r))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_check_version(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let req: protocol::VersionCheckRequest = decode_body(&body)?;
        let server_version = PROTO_API_VERSION;

        if req.version == server_version {
            tracing::info!(
                "Version check passed: machine_id={}, hostname={}, client={}, server={}",
                req.machine_id, req.hostname, req.version, server_version
            );
        } else {
            tracing::warn!(
                "Version check failed: machine_id={}, hostname={}, client={}, server={}",
                req.machine_id, req.hostname, req.version, server_version
            );
        }

        Ok(postcard_response(&protocol::VersionCheckResponse {
            compatible: req.version == server_version,
            server_version: server_version.to_string(),
        }))
    }

    /// Builder POSTs a stream of `BuilderMessage` frames (Join + Pings).
    /// The tunnel is identified by `machine_id` extracted from the first Join message.
    #[tracing::instrument(skip(self, req))]
    async fn handle_tunnel_send(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        use http_body_util::BodyExt as _;

        let (input_tx, mut input_rx) = mpsc::channel::<MachineMessage>(128);
        let use_presigned_uploads = self.state.config.use_presigned_uploads();
        let forced_substituters = self.state.config.get_forced_substituters();

        let mut body = req.into_body();
        let mut reader = protocol::FrameReader::new();

        // Read first frame to get Join message
        let join_msg = loop {
            let Some(frame_result) = body.frame().await else {
                return Err(error_response(400, "stream ended before join message"));
            };
            let frame = frame_result.map_err(|e| error_response(400, &format!("body read error: {e}")))?;
            if let Some(data) = frame.data_ref() {
                reader.extend(data);
            }
            if let Some(payload) = reader.next_frame() {
                let msg: BuilderMessage = postcard::from_bytes(&payload)
                    .map_err(|e| error_response(400, &format!("decode error: {e}")))?;
                match msg {
                    BuilderMessage::Join(join) => break join,
                    _ => return Err(error_response(400, "first message must be Join")),
                }
            }
        };

        let machine = Machine::new(join_msg, input_tx, use_presigned_uploads, &forced_substituters)
            .map_err(|e| {
                tracing::error!("Rejecting new machine creation: {e}");
                error_response(400, "Machine is not valid")
            })?;

        let state = self.state.clone();
        let machine_id = state.insert_machine(machine.clone()).await;
        tracing::info!("Registered new machine: machine_id={machine_id} machine={machine}");

        // Send JoinResponse to the recv endpoint via state
        let (output_tx, output_rx) = mpsc::channel(128);
        {
            let mut tunnels = state.rpc_tunnels.lock();
            tunnels.insert(machine_id, output_rx);
        }

        if let Err(e) = output_tx
            .send(protocol::RunnerMessage::Join(protocol::JoinResponse {
                machine_id: machine_id.to_string(),
                max_concurrent_downloads: state.config.get_max_concurrent_downloads(),
            }))
            .await
        {
            tracing::error!("Failed to send join response machine_id={machine_id} e={e}");
            return Err(error_response(500, "Failed to send join response"));
        }

        // Spawn background task to forward machine messages to output channel and send pings
        let mut ping_interval =
            tokio::time::interval(std::time::Duration::from_secs(BACKWARDS_PING_INTERVAL));
        let state2 = state.clone();
        let machine2 = machine.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = ping_interval.tick() => {
                        let msg = protocol::RunnerMessage::Ping(protocol::SimplePingMessage {
                            message: "ping".into(),
                        });
                        if output_tx.send(msg).await.is_err() {
                            state2.remove_machine(machine_id).await;
                            break;
                        }
                    },
                    msg = input_rx.recv() => {
                        if let Some(msg) = msg {
                            if output_tx.send(msg.into_runner_message()).await.is_err() {
                                tracing::error!("Failed to send message to machine={machine_id}");
                                state2.remove_machine(machine_id).await;
                                break;
                            }
                        } else {
                            state2.remove_machine(machine_id).await;
                            break;
                        }
                    },
                }
            }
        });

        // Continue reading pings from the builder in a background task
        let state3 = state.clone();
        tokio::spawn(async move {
            loop {
                match body.frame().await {
                    Some(Ok(frame)) => {
                        if let Some(data) = frame.data_ref() {
                            reader.extend(data);
                        }
                        while let Some(payload) = reader.next_frame() {
                            match postcard::from_bytes::<BuilderMessage>(&payload) {
                                Ok(BuilderMessage::Ping(ping)) => {
                                    tracing::debug!("new ping: {ping:?}");
                                    handle_ping(&state3, &ping);
                                }
                                Ok(BuilderMessage::Join(_)) => (), // already joined
                                Err(e) => {
                                    tracing::error!("failed to decode builder message: {e}");
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("client disconnected: machine={machine_id} hostname={} err={e}", machine2.hostname);
                        state3.remove_machine(machine_id).await;
                        break;
                    }
                    None => {
                        state3.remove_machine(machine_id).await;
                        break;
                    }
                }
            }
        });

        Ok(hyper::Response::builder()
            .status(200)
            .body(empty_body())
            .expect("response builder should not fail"))
    }

    /// Builder GETs a stream of `RunnerMessage` frames. Query param `machine_id` identifies the tunnel.
    #[tracing::instrument(skip(self, req))]
    async fn handle_tunnel_recv(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let query = req.uri().query().unwrap_or("");
        let machine_id_str = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("machine_id="))
            .ok_or_else(|| error_response(400, "missing machine_id query param"))?;
        let machine_id = uuid::Uuid::parse_str(machine_id_str)
            .map_err(|_| error_response(400, "invalid machine_id"))?;

        let output_rx = {
            let mut tunnels = self.state.rpc_tunnels.lock();
            tunnels
                .remove(&machine_id)
                .ok_or_else(|| error_response(404, "no tunnel for this machine_id"))?
        };

        // Convert the receiver into a streaming response of length-prefixed postcard frames
        let stream = tokio_stream::wrappers::ReceiverStream::new(output_rx);
        let frame_stream = tokio_stream::StreamExt::map(stream, |msg| {
            match frame::encode(&msg) {
                Ok(data) => Ok(Frame::data(Bytes::from(data))),
                Err(e) => {
                    tracing::error!("failed to encode runner message: {e}");
                    // Send empty frame on encode error (should not happen)
                    Ok(Frame::data(Bytes::new()))
                }
            }
        });

        let body = StreamBody::new(frame_stream);
        Ok(hyper::Response::builder()
            .status(200)
            .header("content-type", "application/x-postcard-stream")
            .body(http_body_util::BodyExt::boxed(body))
            .expect("response builder should not fail"))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_build_log(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let state = self.state.clone();
        let mut body = req.into_body();
        let mut reader = protocol::FrameReader::new();
        let mut out_file: Option<fs_err::tokio::File> = None;

        loop {
            match body.frame().await {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        reader.extend(data);
                    }
                    while let Some(payload) = reader.next_frame() {
                        let chunk: protocol::LogChunk = postcard::from_bytes(&payload)
                            .map_err(|e| error_response(400, &format!("decode error: {e}")))?;
                        if let Some(ref mut file) = out_file {
                            file.write_all(&chunk.data)
                                .await
                                .map_err(|e| error_response(500, &format!("write error: {e}")))?;
                        } else {
                            let mut file = state
                                .new_log_file(&nix_utils::parse_store_path(&chunk.drv))
                                .await
                                .map_err(|_| error_response(500, "Failed to create log file"))?;
                            file.write_all(&chunk.data)
                                .await
                                .map_err(|e| error_response(500, &format!("write error: {e}")))?;
                            out_file = Some(file);
                        }
                    }
                }
                Some(Err(e)) => return Err(error_response(500, &format!("body read error: {e}"))),
                None => break,
            }
        }

        Ok(hyper::Response::builder()
            .status(200)
            .body(empty_body())
            .expect("response builder should not fail"))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_build_result(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let mut body = req.into_body();
        let mut reader = protocol::FrameReader::new();

        // Collect all NAR chunks into a stream for import
        let (tx, rx) = mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();

        let import_task = tokio::spawn(async move {
            let store = nix_utils::LocalStore::init();
            store
                .import_paths(tokio_stream::wrappers::UnboundedReceiverStream::new(rx), false)
                .await
        });

        loop {
            match body.frame().await {
                Some(Ok(frame)) => {
                    if let Some(data) = frame.data_ref() {
                        reader.extend(data);
                    }
                    while let Some(payload) = reader.next_frame() {
                        let chunk: protocol::NarData = postcard::from_bytes(&payload)
                            .map_err(|e| error_response(400, &format!("decode error: {e}")))?;
                        let _ = tx.send(Ok(Bytes::from(chunk.chunk)));
                    }
                }
                Some(Err(e)) => {
                    let _ = tx.send(Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, e)));
                    break;
                }
                None => break,
            }
        }
        drop(tx);

        import_task
            .await
            .map_err(|e| error_response(500, &format!("import task join error: {e}")))?
            .map_err(|_| error_response(500, "Failed to import path"))?;

        Ok(hyper::Response::builder()
            .status(200)
            .body(empty_body())
            .expect("response builder should not fail"))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_step_update(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let update: protocol::StepUpdate = decode_body(&body)?;

        let build_id = uuid::Uuid::parse_str(&update.build_id)
            .map_err(|e| error_response(400, &format!("build_id is not a valid uuid: {e}")))?;
        let machine_id = uuid::Uuid::parse_str(&update.machine_id)
            .map_err(|e| error_response(400, &format!("machine_id is not a valid uuid: {e}")))?;
        let step_status = crate::state::step_status_from_protocol(update.step_status);

        let state = self.state.clone();
        tokio::spawn({
            async move {
                if let Err(e) = state.update_build_step(build_id, machine_id, step_status).await {
                    tracing::error!(
                        "Failed to update build step with build_id={build_id:?} step_status={step_status:?}: {e}"
                    );
                }
            }
            .in_current_span()
        });

        Ok(hyper::Response::builder()
            .status(200)
            .body(empty_body())
            .expect("response builder should not fail"))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_complete_build(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let info: protocol::BuildResultInfo = decode_body(&body)?;

        let build_id = uuid::Uuid::parse_str(&info.build_id)
            .map_err(|e| error_response(400, &format!("build_id is not a valid uuid: {e}")))?;
        let machine_id = uuid::Uuid::parse_str(&info.machine_id)
            .map_err(|e| error_response(400, &format!("machine_id is not a valid uuid: {e}")))?;

        let state = self.state.clone();
        let result_state = info.result_state;
        let timings = crate::state::BuildTimings::new(
            info.import_time_ms,
            info.build_time_ms,
            info.upload_time_ms,
        );
        tokio::spawn({
            async move {
                if result_state == protocol::BuildResultState::Success {
                    let build_output =
                        match crate::state::BuildOutput::from_rpc(state.store.store_dir(), info) {
                            Ok(output) => output,
                            Err(e) => {
                                tracing::error!("Failed to parse build output: {e}");
                                return;
                            }
                        };
                    if let Err(e) = state
                        .succeed_step_by_uuid(build_id, machine_id, build_output)
                        .await
                    {
                        tracing::error!(
                            "Failed to mark step with build_id={build_id} as done: {e}"
                        );
                    }
                } else if let Err(e) = state
                    .fail_step_by_uuid(
                        build_id,
                        machine_id,
                        result_state.into(),
                        timings,
                    )
                    .await
                {
                    tracing::error!("Failed to fail step with build_id={build_id}: {e}");
                }
            }
            .in_current_span()
        });

        Ok(hyper::Response::builder()
            .status(200)
            .body(empty_body())
            .expect("response builder should not fail"))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_fetch_requisites(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let fetch: protocol::FetchRequisitesRequest = decode_body(&body)?;

        let drv = nix_utils::parse_store_path(&fetch.path);
        let requisites = self
            .state
            .store
            .query_requisites(&[&drv], fetch.include_outputs)
            .await
            .map_err(|e| {
                tracing::error!("failed to toposort drv e={e}");
                error_response(500, "failed to toposort drv")
            })?
            .into_iter()
            .map(|p| p.to_string())
            .collect();

        Ok(postcard_response(&protocol::DrvRequisitesMessage {
            requisites,
        }))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_has_path(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let path_str: String = decode_body(&body)?;
        let path = nix_utils::parse_store_path(&path_str);
        let has_path = self.state.store.is_valid_path(&path).await;

        Ok(postcard_response(&protocol::HasPathResponse { has_path }))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_stream_file(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let query = req.uri().query().unwrap_or("");
        let path_str = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("path="))
            .ok_or_else(|| error_response(400, "missing path query param"))?;
        let path_str = urlencoding::decode(path_str)
            .map_err(|e| error_response(400, &format!("invalid path encoding: {e}")))?;
        let path = nix_utils::parse_store_path(&path_str);

        let store = nix_utils::LocalStore::init();
        let (tx, rx) = mpsc::unbounded_channel::<Result<Frame<Bytes>, hyper::Error>>();

        tokio::task::spawn(async move {
            let closure = move |data: &[u8]| {
                let chunk = protocol::NarData {
                    chunk: Vec::from(data),
                };
                match frame::encode(&chunk) {
                    Ok(encoded) => tx.send(Ok(Frame::data(Bytes::from(encoded)))).is_ok(),
                    Err(_) => false,
                }
            };
            let _ = store.export_paths(&[path], closure);
        });

        let body = StreamBody::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));
        Ok(hyper::Response::builder()
            .status(200)
            .header("content-type", "application/x-postcard-stream")
            .body(http_body_util::BodyExt::boxed(body))
            .expect("response builder should not fail"))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_stream_files(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let paths_strs: Vec<String> = decode_body(&body)?;
        let paths = paths_strs
            .iter()
            .map(|p| nix_utils::parse_store_path(p))
            .collect::<Vec<_>>();

        let store = nix_utils::LocalStore::init();
        let (tx, rx) = mpsc::unbounded_channel::<Result<Frame<Bytes>, hyper::Error>>();

        let closure = move |data: &[u8]| {
            let chunk = protocol::NarData {
                chunk: Vec::from(data),
            };
            match frame::encode(&chunk) {
                Ok(encoded) => tx.send(Ok(Frame::data(Bytes::from(encoded)))).is_ok(),
                Err(_) => false,
            }
        };

        tokio::task::spawn(async move {
            let _ = store.export_paths(&paths, closure);
        });

        let body = StreamBody::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));
        Ok(hyper::Response::builder()
            .status(200)
            .header("content-type", "application/x-postcard-stream")
            .body(http_body_util::BodyExt::boxed(body))
            .expect("response builder should not fail"))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_presigned_url(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let presigned_req: protocol::PresignedUrlRequest = decode_body(&body)?;

        let _build_id = uuid::Uuid::parse_str(&presigned_req.build_id)
            .map_err(|e| error_response(400, &format!("build_id is not a valid uuid: {e}")))?;
        let _machine_id = uuid::Uuid::parse_str(&presigned_req.machine_id)
            .map_err(|e| error_response(400, &format!("machine_id is not a valid uuid: {e}")))?;

        let remote_store = {
            let remote_stores = self.state.remote_stores.read();
            remote_stores
                .iter()
                .find_map(|s| match s {
                    crate::state::RemoteStoreBackend::S3(s) => Some(s.clone()),
                    _ => None,
                })
                .ok_or_else(|| error_response(412, "No remote store configured"))?
        };

        let mut responses = Vec::new();
        for presigned_request in presigned_req.request {
            let store_path = nix_utils::parse_store_path(&presigned_request.store_path);

            let presigned_response = remote_store
                .generate_nar_upload_presigned_url(
                    &store_path,
                    &presigned_request.nar_hash,
                    presigned_request.debug_info_build_ids,
                )
                .await
                .map_err(|e| {
                    tracing::error!("Failed to generate presigned URL for {}: {e}", store_path);
                    error_response(500, "Failed to generate presigned URL")
                })?;

            responses.push(protocol::PresignedNarResponse {
                store_path: store_path.to_string().to_owned(),
                nar_url: presigned_response.nar_url,
                nar_upload: protocol::PresignedUpload {
                    compression_level: presigned_response.nar_upload.get_compression_level_as_i32(),
                    url: presigned_response.nar_upload.url,
                    path: presigned_response.nar_upload.path,
                    compression: presigned_response
                        .nar_upload
                        .compression
                        .as_str()
                        .to_owned(),
                },
                ls_upload: presigned_response
                    .ls_upload
                    .map(|ls| protocol::PresignedUpload {
                        compression_level: ls.get_compression_level_as_i32(),
                        url: ls.url,
                        path: ls.path,
                        compression: ls.compression.as_str().to_owned(),
                    }),
                debug_info_upload: presigned_response
                    .debug_info_upload
                    .into_iter()
                    .map(|p| protocol::PresignedUpload {
                        compression_level: p.get_compression_level_as_i32(),
                        url: p.url,
                        path: p.path,
                        compression: p.compression.as_str().to_owned(),
                    })
                    .collect(),
            });
        }

        tracing::debug!("Generated {} presigned URLs", responses.len());
        Ok(postcard_response(&protocol::PresignedUrlResponse {
            inner: responses,
        }))
    }

    #[tracing::instrument(skip(self, req))]
    async fn handle_presigned_complete(
        &self,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<BoxBody>, hyper::Response<BoxBody>> {
        let body = read_body(req).await?;
        let completion: protocol::PresignedUploadComplete = decode_body(&body)?;

        let build_id = uuid::Uuid::parse_str(&completion.build_id)
            .map_err(|e| error_response(400, &format!("build_id is not a valid uuid: {e}")))?;
        let machine_id = uuid::Uuid::parse_str(&completion.machine_id)
            .map_err(|e| error_response(400, &format!("machine_id is not a valid uuid: {e}")))?;

        let machine = self
            .state
            .machines
            .get_machine_by_id(machine_id)
            .ok_or_else(|| error_response(404, "Machine not found"))?;
        let _job = machine
            .get_job_drv_for_build_id(build_id)
            .ok_or_else(|| error_response(404, "Job not found for this build_id"))?;

        let remote_store = {
            let remote_stores = self.state.remote_stores.read();
            remote_stores
                .iter()
                .find_map(|s| match s {
                    crate::state::RemoteStoreBackend::S3(s) => Some(s.clone()),
                    _ => None,
                })
                .ok_or_else(|| error_response(412, "No remote store configured"))?
        };

        let narinfo = binary_cache::NarInfo {
            store_path: nix_utils::parse_store_path(&completion.store_path),
            url: completion.url.clone(),
            compression: remote_store.cfg.compression,
            file_hash: Some(completion.file_hash),
            file_size: Some(completion.file_size),
            nar_hash: completion.nar_hash,
            nar_size: completion.nar_size,
            references: completion
                .references
                .into_iter()
                .map(|p| nix_utils::parse_store_path(&p))
                .collect(),
            deriver: completion
                .deriver
                .map(|p| nix_utils::parse_store_path(&p)),
            ca: completion.ca,
            sigs: vec![],
        };
        let store_path = narinfo.store_path.clone();

        let narinfo_url = remote_store
            .upload_narinfo_after_presigned_upload(&self.state.store, narinfo)
            .await
            .map_err(|e| {
                tracing::error!("Failed to upload narinfo for {}: {e}", store_path);
                error_response(500, "Failed to upload narinfo")
            })?;

        tracing::debug!(
            "Presigned upload completed and narinfo uploaded for path: {}, url: {}, size: {} bytes, narinfo: {}",
            store_path,
            completion.url,
            completion.file_size,
            narinfo_url
        );

        Ok(hyper::Response::builder()
            .status(200)
            .body(empty_body())
            .expect("response builder should not fail"))
    }
}
