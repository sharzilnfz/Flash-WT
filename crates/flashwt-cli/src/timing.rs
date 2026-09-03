use std::time::Instant;

#[derive(Default)]
pub struct StageTimings {
    pub git_worktree_ms: u128,
    pub ingest_ms: u128,
    pub references_ms: u128,
    pub materialize_ms: u128,

    pub verify_ms: u128,
    pub place_ms: u128,
    pub snapshot_ms: u128,

    pub snapshot_engaged: bool,
    pub snapshot_lookup_ms: u128,
    pub snapshot_clonefile_ms: u128,

    pub build_verify_ms: u64,
    pub build_link_train_ms: u64,
    pub build_publish_ms: u64,
    pub snapshot_built: bool,

    pub snapshot_mode: &'static str,
    pub v2_cloned: usize,
    pub v2_linked: usize,
}

impl StageTimings {
    pub fn new() -> Self {
        Self {
            snapshot_mode: "build",
            ..Self::default()
        }
    }

    pub fn emit(&self, started: Instant, enabled: bool) {
        if !enabled {
            return;
        }
        eprintln!("flashwt-stage git-worktree={}", self.git_worktree_ms);
        eprintln!("flashwt-stage ingest={}", self.ingest_ms);
        eprintln!("flashwt-stage references={}", self.references_ms);
        eprintln!("flashwt-stage materialize={}", self.materialize_ms);
        if self.materialize_ms > 0 {
            eprintln!("flashwt-stage verify={}", self.verify_ms);
            eprintln!("flashwt-stage place={}", self.place_ms);
        }
        if self.snapshot_engaged {
            eprintln!("flashwt-stage snapshot={}", self.snapshot_ms);
            eprintln!("flashwt-stage snapshot-lookup={}", self.snapshot_lookup_ms);
            eprintln!(
                "flashwt-stage snapshot-clonefile={}",
                self.snapshot_clonefile_ms
            );
            eprintln!("flashwt-stage snapshot-mode={}", self.snapshot_mode);
            eprintln!("flashwt-stage snapshot-v2-cloned={}", self.v2_cloned);
            eprintln!("flashwt-stage snapshot-v2-linked={}", self.v2_linked);
            if self.snapshot_built {
                eprintln!(
                    "flashwt-stage snapshot-build-verify={}",
                    self.build_verify_ms
                );
                eprintln!(
                    "flashwt-stage snapshot-build-link-train={}",
                    self.build_link_train_ms
                );
                eprintln!(
                    "flashwt-stage snapshot-build-publish={}",
                    self.build_publish_ms
                );
            }
        }

        eprintln!("flashwt-stage total={}", started.elapsed().as_millis());
    }
}
