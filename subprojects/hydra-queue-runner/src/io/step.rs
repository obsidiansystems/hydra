/// IO representation of a step, for serialization to JSON.
/// Now populated from database queries rather than in-memory `Step` objects.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    drv_path: String,
    ready_time: i32,
    highest_global_priority: i32,
    highest_local_priority: i32,
    lowest_build_id: db::models::BuildID,
    rdeps_count: i64,
}

impl From<db::models::DispatchCandidate> for Step {
    fn from(item: db::models::DispatchCandidate) -> Self {
        Self {
            drv_path: item.drv_path,
            ready_time: item.ready_time,
            highest_global_priority: item.highest_global_priority,
            highest_local_priority: item.highest_local_priority,
            lowest_build_id: item.lowest_build_id,
            rdeps_count: item.rdeps_count,
        }
    }
}
