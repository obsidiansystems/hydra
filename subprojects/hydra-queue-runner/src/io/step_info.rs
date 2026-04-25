/// IO representation of a scheduled item, for serialization to JSON.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepInfo {
    drv_path: nix_utils::StorePath,
    machine_hostname: String,
}

impl From<crate::state::queue::ScheduledItem> for StepInfo {
    fn from(item: crate::state::queue::ScheduledItem) -> Self {
        Self {
            drv_path: item.drv_path,
            machine_hostname: item.machine.hostname.clone(),
        }
    }
}
