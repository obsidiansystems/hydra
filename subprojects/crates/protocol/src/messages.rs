use serde::{Deserialize, Serialize};

// -- Version check --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCheckRequest {
    pub version: String,
    pub machine_id: String,
    pub hostname: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCheckResponse {
    pub compatible: bool,
    pub server_version: String,
}

// -- Builder→Runner tunnel messages --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BuilderMessage {
    Join(JoinMessage),
    Ping(PingMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinMessage {
    pub machine_id: String,
    pub systems: Vec<String>,
    pub hostname: String,
    pub cpu_count: u32,
    pub bogomips: f32,
    pub speed_factor: f32,
    pub max_jobs: u32,
    pub build_dir_avail_threshold: f32,
    pub store_avail_threshold: f32,
    pub load1_threshold: f32,
    pub cpu_psi_threshold: f32,
    pub mem_psi_threshold: f32,
    pub io_psi_threshold: Option<f32>,
    pub total_mem: u64,
    pub supported_features: Vec<String>,
    pub mandatory_features: Vec<String>,
    pub cgroups: bool,
    pub substituters: Vec<String>,
    pub use_substitutes: bool,
    pub nix_version: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Pressure {
    pub avg10: f32,
    pub avg60: f32,
    pub avg300: f32,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PressureState {
    pub cpu_some: Option<Pressure>,
    pub mem_some: Option<Pressure>,
    pub mem_full: Option<Pressure>,
    pub io_some: Option<Pressure>,
    pub io_full: Option<Pressure>,
    pub irq_full: Option<Pressure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingMessage {
    pub machine_id: String,
    pub load1: f32,
    pub load5: f32,
    pub load15: f32,
    pub mem_usage: u64,
    pub pressure: Option<PressureState>,
    pub build_dir_free_percent: f64,
    pub store_free_percent: f64,
    pub current_substituting_path_count: u64,
    pub current_uploading_path_count: u64,
    pub current_downloading_path_count: u64,
}

// -- Runner→Builder tunnel messages --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunnerMessage {
    Join(JoinResponse),
    ConfigUpdate(ConfigUpdate),
    Ping(SimplePingMessage),
    Build(BuildMessage),
    Abort(AbortMessage),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinResponse {
    pub machine_id: String,
    pub max_concurrent_downloads: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigUpdate {
    pub max_concurrent_downloads: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimplePingMessage {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedUploadOpts {
    pub upload_debug_info: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildMessage {
    pub build_id: String,
    pub drv: String,
    pub resolved_drv: Option<String>,
    pub max_log_size: u64,
    pub max_silent_time: i32,
    pub build_timeout: i32,
    pub presigned_url_opts: Option<PresignedUploadOpts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortMessage {
    pub build_id: String,
}

// -- Build log --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogChunk {
    pub drv: String,
    pub data: Vec<u8>,
}

// -- Requisites --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchRequisitesRequest {
    pub path: String,
    pub include_outputs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrvRequisitesMessage {
    pub requisites: Vec<String>,
}

// -- Store paths --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HasPathResponse {
    pub has_path: bool,
}

// -- NAR data (streaming chunk) --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarData {
    pub chunk: Vec<u8>,
}

// -- Build outputs --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Output {
    NameOnly { name: String },
    WithPath(OutputWithPath),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputWithPath {
    pub name: String,
    pub path: String,
    pub closure_size: u64,
    pub nar_size: u64,
    pub nar_hash: String,
}

// -- Build metrics / products --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildMetric {
    pub path: String,
    pub name: String,
    pub unit: Option<String>,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProduct {
    pub path: String,
    pub default_path: String,
    pub r#type: String,
    pub subtype: String,
    pub name: String,
    pub is_regular: bool,
    pub sha256hash: Option<String>,
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NixSupport {
    pub failed: bool,
    pub hydra_release_name: Option<String>,
    pub metrics: Vec<BuildMetric>,
    pub products: Vec<BuildProduct>,
}

// -- Step status --

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum StepStatus {
    Preparing = 0,
    Connecting = 1,
    SendingInputs = 2,
    Building = 3,
    WaitingForLocalSlot = 4,
    ReceivingOutputs = 5,
    PostProcessing = 6,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepUpdate {
    pub build_id: String,
    pub machine_id: String,
    pub step_status: StepStatus,
}

// -- Build result --

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum BuildResultState {
    BuildFailure = 0,
    Success = 1,
    PreparingFailure = 2,
    ImportFailure = 3,
    UploadFailure = 4,
    PostProcessingFailure = 5,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResultInfo {
    pub build_id: String,
    pub machine_id: String,
    pub import_time_ms: u64,
    pub build_time_ms: u64,
    pub upload_time_ms: u64,
    pub result_state: BuildResultState,
    pub nix_support: Option<NixSupport>,
    pub outputs: Vec<Output>,
}

// -- Presigned URL --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedNarRequest {
    pub store_path: String,
    pub nar_hash: String,
    pub debug_info_build_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedUrlRequest {
    pub build_id: String,
    pub machine_id: String,
    pub request: Vec<PresignedNarRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedUpload {
    pub path: String,
    pub url: String,
    pub compression: String,
    pub compression_level: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedNarResponse {
    pub store_path: String,
    pub nar_url: String,
    pub nar_upload: PresignedUpload,
    pub ls_upload: Option<PresignedUpload>,
    pub debug_info_upload: Vec<PresignedUpload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedUrlResponse {
    pub inner: Vec<PresignedNarResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresignedUploadComplete {
    pub build_id: String,
    pub machine_id: String,
    pub store_path: String,
    pub url: String,
    pub compression: String,
    pub file_hash: String,
    pub file_size: u64,
    pub nar_hash: String,
    pub nar_size: u64,
    pub references: Vec<String>,
    pub deriver: Option<String>,
    pub ca: Option<String>,
}
