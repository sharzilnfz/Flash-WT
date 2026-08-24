//! Per-stage wall-clock attribution for `wt create` (Step 0
//! instrumentation, decomposed from main.rs by arch-hardening ticket
//! 03). One line per stage on stderr (`wt-stage <name>=<ms>`, integer
//! milliseconds) behind `WT_TIMING=1`.

use std::time::Instant;

/// Every accumulator the create loop feeds, replacing the former emit
/// closure's fourteen positional arguments and fifteen locals.
#[derive(Default)]
pub struct StageTimings {
    pub git_worktree_ms: u128,
    pub ingest_ms: u128,
    pub references_ms: u128,
    pub materialize_ms: u128,
    /// Fine-grained sub-stage attribution (Step 0).
    pub verify_ms: u128,
    pub place_ms: u128,
    pub snapshot_ms: u128,
    /// Whether the snapshot fast path did any work at all.
    pub snapshot_engaged: bool,
    pub snapshot_lookup_ms: u128,
    pub snapshot_clonefile_ms: u128,
    /// Internal build-phase timings; only meaningful once a run built
    /// and published a snapshot.
    pub build_verify_ms: u64,
    pub build_link_train_ms: u64,
    pub build_publish_ms: u64,
    pub snapshot_built: bool,
    /// v2 incremental reporting: the mode line shows the LAST heavy
    /// directory's serving mode (hit/build/v2); the counters sum.
    pub snapshot_mode: &'static str,
    pub v2_cloned: usize,
    pub v2_linked: usize,
}

impl StageTimings {
    /// `snapshot_mode` starts as `"build"` (the mode a plain per-file
    /// run would report); everything else is zeroed.
    pub fn new() -> Self {
        Self {
            snapshot_mode: "build",
            ..Self::default()
        }
    }

    /// The snapshot line appears only when the fast path did work, so
    /// pre-snapshot consumers see the same four lines as before; its
    /// meaning is unchanged (lookup + build + clone wall time).
    pub fn emit(&self, started: Instant, enabled: bool) {
        if !enabled {
            return;
        }
        eprintln!("wt-stage git-worktree={}", self.git_worktree_ms);
        eprintln!("wt-stage ingest={}", self.ingest_ms);
        eprintln!("wt-stage references={}", self.references_ms);
        eprintln!("wt-stage materialize={}", self.materialize_ms);
        if self.materialize_ms > 0 {
            eprintln!("wt-stage verify={}", self.verify_ms);
            eprintln!("wt-stage place={}", self.place_ms);
        }
        if self.snapshot_engaged {
            eprintln!("wt-stage snapshot={}", self.snapshot_ms);
            eprintln!("wt-stage snapshot-lookup={}", self.snapshot_lookup_ms);
            eprintln!("wt-stage snapshot-clonefile={}", self.snapshot_clonefile_ms);
            eprintln!("wt-stage snapshot-mode={}", self.snapshot_mode);
            eprintln!("wt-stage snapshot-v2-cloned={}", self.v2_cloned);
            eprintln!("wt-stage snapshot-v2-linked={}", self.v2_linked);
            if self.snapshot_built {
                eprintln!("wt-stage snapshot-build-verify={}", self.build_verify_ms);
                eprintln!(
                    "wt-stage snapshot-build-link-train={}",
                    self.build_link_train_ms
                );
                eprintln!("wt-stage snapshot-build-publish={}", self.build_publish_ms);
            }
        }
        // total spans git-worktree-add through summary printing (the
        // git worktree add itself gets its own stage line above).
        eprintln!("wt-stage total={}", started.elapsed().as_millis());
    }
}
