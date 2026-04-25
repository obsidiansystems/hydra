use db::models::BuildID;
use nix_utils::SingleDerivedPath;

/// Flatten a [`SingleDerivedPath`] + output name into `(root_drv_path, [outputs...])`.
/// The output chain is in resolution order: for `Built { Opaque(A), "out" }` with
/// final output `"dev"`, returns `(A, ["out", "dev"])`.
fn flatten_chain(
    drv_path: &SingleDerivedPath,
    output_name: &nix_utils::OutputName,
) -> (nix_utils::StorePath, Vec<nix_utils::OutputName>) {
    let mut outputs = Vec::<nix_utils::OutputName>::new();
    let mut current = drv_path;
    let root = loop {
        match current {
            SingleDerivedPath::Opaque(p) => break p.clone(),
            SingleDerivedPath::Built {
                drv_path: parent,
                output,
            } => {
                outputs.push(output.clone());
                current = parent;
            }
        }
    };
    outputs.reverse();
    outputs.push(output_name.clone());
    (root, outputs)
}

/// Entry representing a step that is ready for dispatch.
/// All scheduling data comes from the database via `DispatchCandidate`.
#[derive(Debug)]
pub struct DispatchEntry {
    pub drv_path: nix_utils::StorePath,
    pub resolved_drv_path: Option<nix_utils::StorePath>,
    pub system: String,
    pub required_features: Vec<String>,
    // Scheduling fields from DispatchCandidate:
    pub ready_time: i32,
    pub highest_global_priority: i32,
    pub highest_local_priority: i32,
    pub lowest_build_id: BuildID,
    pub rdeps_count: i64,
    pub lowest_share_used: f64,
}

impl DispatchEntry {
    pub(super) fn legacy_compare(&self, other: &Self) -> std::cmp::Ordering {
        #[allow(irrefutable_let_patterns)]
        (if let c1 = self
            .highest_global_priority
            .cmp(&other.highest_global_priority)
            && c1 != std::cmp::Ordering::Equal
        {
            c1
        } else if let c2 = other.lowest_share_used.total_cmp(&self.lowest_share_used)
            && c2 != std::cmp::Ordering::Equal
        {
            c2
        } else if let c3 = self
            .highest_local_priority
            .cmp(&other.highest_local_priority)
            && c3 != std::cmp::Ordering::Equal
        {
            c3
        } else {
            other.lowest_build_id.cmp(&self.lowest_build_id)
        })
        .reverse()
    }

    pub(super) fn compare_with_rdeps(&self, other: &Self) -> std::cmp::Ordering {
        #[allow(irrefutable_let_patterns)]
        (if let c1 = self
            .highest_global_priority
            .cmp(&other.highest_global_priority)
            && c1 != std::cmp::Ordering::Equal
        {
            c1
        } else if let c2 = other.lowest_share_used.total_cmp(&self.lowest_share_used)
            && c2 != std::cmp::Ordering::Equal
        {
            c2
        } else if let c3 = self.rdeps_count.cmp(&other.rdeps_count)
            && c3 != std::cmp::Ordering::Equal
        {
            c3
        } else if let c4 = self
            .highest_local_priority
            .cmp(&other.highest_local_priority)
            && c4 != std::cmp::Ordering::Equal
        {
            c4
        } else {
            other.lowest_build_id.cmp(&self.lowest_build_id)
        })
        .reverse()
    }
}

/// Resolve a derivation's inputs into concrete store paths, returning a
/// [`BasicDerivation`](nix_utils::BasicDerivation).
///
/// Returns [`None`] if the derivation is input-addressed (shouldn't be resolved),
/// or if resolution fails because required outputs haven't been built yet.
///
/// We only need a store dir, not a store, because all the info we need comes from the Hydra
/// database.
pub(super) async fn try_resolve(
    store_dir: &nix_utils::StoreDir,
    db: &db::Database,
    drv: &nix_utils::Derivation,
) -> Option<nix_utils::BasicDerivation> {
    // Input-addressed derivations should not be resolved because this would change their
    // output paths.
    let all_input_addressed = drv
        .outputs
        .values()
        .any(|o| matches!(o, nix_utils::DerivationOutput::InputAddressed(_)));
    if all_input_addressed {
        return None;
    }

    // If there are no Built inputs, the derivation is already resolved.
    let has_built_inputs = drv
        .inputs
        .iter()
        .any(|i| matches!(i, SingleDerivedPath::Built { .. }));
    if !has_built_inputs {
        return Some(drv.clone().map_inputs(|inputs| {
            inputs
                .into_iter()
                .map(|sdp| match sdp {
                    SingleDerivedPath::Opaque(p) => p,
                    SingleDerivedPath::Built { .. } => unreachable!(),
                })
                .collect()
        }));
    }

    let mut conn = db.get().await.ok()?;

    drv.try_resolve(store_dir, &mut |inputs| {
        tokio::task::block_in_place(|| {
            // Flatten each SingleDerivedPath chain into (root, [outputs...])
            // and resolve everything in a single recursive SQL query.
            let chains = inputs
                .iter()
                .map(|(drv_path, output_name)| flatten_chain(drv_path, output_name))
                .collect::<Vec<_>>();

            let chain_refs = chains
                .iter()
                .map(|(root, outputs)| (root, outputs.iter().collect::<Vec<_>>()))
                .collect::<Vec<_>>();

            let sql_input = chain_refs
                .iter()
                .map(|(root, outputs)| (*root, outputs.as_slice()))
                .collect::<Vec<_>>();

            tokio::runtime::Handle::current()
                .block_on(conn.resolve_drv_output_chains(store_dir, &sql_input))
                .unwrap_or_else(|e| {
                    tracing::warn!("resolve_drv_output_chains failed: {e}");
                    vec![None; inputs.len()]
                })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entry(
        highest_global_priority: i32,
        highest_local_priority: i32,
        lowest_build_id: BuildID,
        lowest_share_used: f64,
        rdeps_count: i64,
    ) -> DispatchEntry {
        DispatchEntry {
            drv_path: nix_utils::parse_store_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-test.drv"),
            resolved_drv_path: None,
            system: "x86_64-linux".to_string(),
            required_features: vec![],
            ready_time: 0,
            highest_global_priority,
            highest_local_priority,
            lowest_build_id,
            rdeps_count,
            lowest_share_used,
        }
    }

    #[test]
    fn test_legacy_compare_global_priority() {
        let step1 = create_test_entry(10, 1, 1, 1.0, 0);
        let step2 = create_test_entry(5, 1, 2, 1.0, 0);

        assert_eq!(step1.legacy_compare(&step2), std::cmp::Ordering::Less);
        assert_eq!(step2.legacy_compare(&step1), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_legacy_compare_share_used() {
        let step1 = create_test_entry(5, 1, 1, 0.5, 0);
        let step2 = create_test_entry(5, 1, 2, 1.0, 0);

        assert_eq!(step1.legacy_compare(&step2), std::cmp::Ordering::Less);
        assert_eq!(step2.legacy_compare(&step1), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_legacy_compare_local_priority() {
        let step1 = create_test_entry(5, 10, 1, 1.0, 0);
        let step2 = create_test_entry(5, 5, 2, 1.0, 0);

        assert_eq!(step1.legacy_compare(&step2), std::cmp::Ordering::Less);
        assert_eq!(step2.legacy_compare(&step1), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_legacy_compare_build_id() {
        let step1 = create_test_entry(5, 1, 1, 1.0, 0);
        let step2 = create_test_entry(5, 1, 2, 1.0, 0);

        assert_eq!(step1.legacy_compare(&step2), std::cmp::Ordering::Less);
        assert_eq!(step2.legacy_compare(&step1), std::cmp::Ordering::Greater);
    }

    #[test]
    fn test_legacy_compare_equal() {
        let step1 = create_test_entry(5, 1, 1, 1.0, 0);
        let step2 = create_test_entry(5, 1, 1, 1.0, 0);

        assert_eq!(step1.legacy_compare(&step2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_compare_with_rdeps_global_priority() {
        let step1 = create_test_entry(10, 1, 1, 1.0, 0);
        let step2 = create_test_entry(5, 1, 2, 1.0, 0);

        assert_eq!(step1.compare_with_rdeps(&step2), std::cmp::Ordering::Less);
        assert_eq!(
            step2.compare_with_rdeps(&step1),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_with_rdeps_share_used() {
        let step1 = create_test_entry(5, 1, 1, 0.5, 0);
        let step2 = create_test_entry(5, 1, 2, 1.0, 0);

        assert_eq!(step1.compare_with_rdeps(&step2), std::cmp::Ordering::Less);
        assert_eq!(
            step2.compare_with_rdeps(&step1),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_with_rdeps_rdeps_len() {
        let step1 = create_test_entry(5, 1, 1, 1.0, 10);
        let step2 = create_test_entry(5, 1, 2, 1.0, 5);

        assert_eq!(step1.compare_with_rdeps(&step2), std::cmp::Ordering::Less);
        assert_eq!(
            step2.compare_with_rdeps(&step1),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_with_rdeps_local_priority() {
        let step1 = create_test_entry(5, 10, 1, 1.0, 0);
        let step2 = create_test_entry(5, 5, 2, 1.0, 0);

        assert_eq!(step1.compare_with_rdeps(&step2), std::cmp::Ordering::Less);
        assert_eq!(
            step2.compare_with_rdeps(&step1),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_with_rdeps_build_id() {
        let step1 = create_test_entry(5, 1, 1, 1.0, 0);
        let step2 = create_test_entry(5, 1, 2, 1.0, 0);

        assert_eq!(step1.compare_with_rdeps(&step2), std::cmp::Ordering::Less);
        assert_eq!(
            step2.compare_with_rdeps(&step1),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_compare_with_rdeps_equal() {
        let step1 = create_test_entry(5, 1, 1, 1.0, 0);
        let step2 = create_test_entry(5, 1, 1, 1.0, 0);

        assert_eq!(step1.compare_with_rdeps(&step2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_difference_between_compare_functions() {
        let step1 = create_test_entry(5, 1, 1, 1.0, 10);
        let step2 = create_test_entry(5, 1, 1, 1.0, 5);

        assert_eq!(step1.legacy_compare(&step2), std::cmp::Ordering::Equal);

        assert_eq!(step1.compare_with_rdeps(&step2), std::cmp::Ordering::Less);
        assert_eq!(
            step2.compare_with_rdeps(&step1),
            std::cmp::Ordering::Greater
        );
    }
}
