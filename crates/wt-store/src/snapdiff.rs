//! Manifest diff for v2 incremental snapshot rebuilds.
//!
//! Both inputs are canonical manifests (sorted by raw relpath bytes,
//! then kind), so a single sorted merge classifies every path:
//!
//! - **unchanged**: present in both with the same kind, mode, and ref
//!   (blob id for files, target string for symlinks; dirs carry none).
//! - **modified**: same relpath and kind, different mode or ref. A
//!   kind flip is treated as delete + add: placement differs too much
//!   to call it a modification.
//! - **added** / **deleted**: present on one side only.
//!
//! On top of the classification sits the UNCHANGED SUBTREE UNIT rule:
//! a directory `D` is fully unchanged iff it appears as a dir entry in
//! both manifests and every entry at or under `D/` — in EITHER
//! manifest — is unchanged. Units are computed bottom-up but reported
//! MAXIMAL only: if `D` qualifies, its qualifying descendants are
//! folded into it and never listed separately. Unchanged files at the
//! manifest root are not units; they are linked individually like any
//! other non-unit content.

use std::collections::{HashMap, HashSet};

use crate::snapshot::SnapshotEntry;

/// Classification of one rebuild against an old manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotDiff {
    /// Entries present only in the new manifest.
    pub added: Vec<SnapshotEntry>,
    /// Entries in both whose content address changed.
    pub modified: Vec<SnapshotEntry>,
    /// Entries present only in the old manifest.
    pub deleted: Vec<SnapshotEntry>,
}

impl SnapshotDiff {
    /// Sorted merge of two canonically ordered flat entry lists.
    #[must_use]
    pub fn compute(old: &[SnapshotEntry], new: &[SnapshotEntry]) -> SnapshotDiff {
        let mut diff = SnapshotDiff::default();
        let (mut i, mut j) = (0usize, 0usize);
        while i < old.len() && j < new.len() {
            match old[i].rel.as_bytes().cmp(new[j].rel.as_bytes()) {
                std::cmp::Ordering::Less => {
                    diff.deleted.push(old[i].clone());
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    diff.added.push(new[j].clone());
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    if !entries_identical(&old[i], &new[j]) {
                        if old[i].kind == new[j].kind {
                            diff.modified.push(new[j].clone());
                        } else {
                            // Kind flip: delete + add, never modify.
                            diff.deleted.push(old[i].clone());
                            diff.added.push(new[j].clone());
                        }
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        diff.deleted.extend_from_slice(&old[i..]);
        diff.added.extend_from_slice(&new[j..]);
        diff
    }

    /// Entries needing any work at all. The v2 gate uses this as its
    /// cost heuristic against the NEW entry count.
    #[must_use]
    pub fn changed_count(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }

    /// Maximal fully-unchanged directory units between two canonical
    /// manifests. Pure function; see the module docs for the rule.
    ///
    /// Complexity: one merge for the unchanged set, one sort of the
    /// merged rels, then O(log n) range lookups per candidate
    /// directory (prefix sums over dirtiness). The previous
    /// implementation rescanned every entry under each directory, which
    /// was quadratic in entries x directories on v2's hot path.
    #[must_use]
    pub fn unchanged_units(old: &[SnapshotEntry], new: &[SnapshotEntry]) -> Vec<String> {
        let mut same: HashSet<&str> = HashSet::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < old.len() && j < new.len() {
            match old[i].rel.as_bytes().cmp(new[j].rel.as_bytes()) {
                std::cmp::Ordering::Less => i += 1,
                std::cmp::Ordering::Greater => j += 1,
                std::cmp::Ordering::Equal => {
                    if entries_identical(&old[i], &new[j]) {
                        same.insert(old[i].rel.as_str());
                    }
                    i += 1;
                    j += 1;
                }
            }
        }

        // Merged, deduped rel list in canonical order.
        let mut merged: Vec<&str> = Vec::with_capacity(old.len() + new.len());
        merged.extend(old.iter().map(|e| e.rel.as_str()));
        merged.extend(new.iter().map(|e| e.rel.as_str()));
        merged.sort_unstable();
        merged.dedup();

        // Prefix sums over "dirty" (not identical on both sides): an
        // entry is dirty iff its rel is absent from `same` — including
        // rels present on only one side. dirty_prefix[k] counts dirty
        // entries among merged[..k].
        let mut dirty_prefix = vec![0usize; merged.len() + 1];
        for (k, rel) in merged.iter().enumerate() {
            dirty_prefix[k + 1] = dirty_prefix[k] + usize::from(!same.contains(rel));
        }

        // Directories present as dir entries on BOTH sides.
        let old_dirs: HashSet<&str> = old
            .iter()
            .filter(|e| e.kind == crate::snapshot::EntryKind::Dir)
            .map(|e| e.rel.as_str())
            .collect();
        let new_dirs: HashSet<&str> = new
            .iter()
            .filter(|e| e.kind == crate::snapshot::EntryKind::Dir)
            .map(|e| e.rel.as_str())
            .collect();

        // Deepest first so maximal units suppress their qualifying
        // descendants. Stable sort keeps lexicographic order within
        // equal lengths, exactly like the original pipeline.
        let mut dirs: Vec<&str> = merged
            .iter()
            .copied()
            .filter(|rel| old_dirs.contains(rel) && new_dirs.contains(rel))
            .collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));

        // A dir d is fully unchanged iff d itself is unchanged AND no
        // entry under d/ is dirty. All entries carrying the d/
        // prefix sort in the half-open range ["d/", "d0"): '/' + 1
        // is '0', so the successor bound is just an appended '0'.
        let fully: HashMap<&str, bool> = dirs
            .iter()
            .map(|d| {
                let prefix = format!("{d}/");
                let bound = format!("{d}0");
                let start = merged.partition_point(|rel| rel.as_bytes() < prefix.as_bytes());
                let end = merged.partition_point(|rel| rel.as_bytes() < bound.as_bytes());
                debug_assert!(end >= start);
                let clean_range = end >= start && dirty_prefix[end] - dirty_prefix[start] == 0;
                (*d, clean_range && same.contains(d))
            })
            .collect();

        // A unit must be fully unchanged AND have no fully-unchanged
        // ancestor (otherwise it is part of that bigger unit). The
        // ancestor walk terminates because relpaths have bounded depth.
        dirs.into_iter()
            .filter(|d| fully[d])
            .filter(|d| {
                !ancestor_dirs(d)
                    .into_iter()
                    .any(|a| fully.get(a.as_str()) == Some(&true))
            })
            .map(str::to_owned)
            .collect()
    }
}

fn entries_identical(a: &SnapshotEntry, b: &SnapshotEntry) -> bool {
    a.kind == b.kind && a.mode == b.mode && a.blob == b.blob && a.target == b.target
}

/// Proper directory ancestors of a relpath, nearest first:
/// `"deep/a/b"` -> `["deep/a", "deep"]`.
fn ancestor_dirs(rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = rel;
    while let Some(pos) = cur.rfind('/') {
        cur = &cur[..pos];
        out.push(cur.to_string());
    }
    out
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{EntryKind, SnapshotEntry};
    use crate::ContentId;

    fn id(n: u8) -> ContentId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        bytes[31] = n;
        ContentId(bytes)
    }

    /// Deterministic per-name content so two `pkg(name)` calls always
    /// produce identical manifests.
    fn pkg(name: &str) -> Vec<SnapshotEntry> {
        let blob = (name.as_bytes()[name.len() - 1] - b'0') * 10;
        vec![
            SnapshotEntry::dir(name),
            SnapshotEntry::file(format!("{name}/a"), id(blob), 0o644),
            SnapshotEntry::file(format!("{name}/b"), id(blob + 1), 0o644),
        ]
    }

    fn sorted(mut v: Vec<SnapshotEntry>) -> Vec<SnapshotEntry> {
        v.sort_by(|a, b| {
            a.rel
                .as_bytes()
                .cmp(b.rel.as_bytes())
                .then_with(|| a.kind.cmp(&b.kind))
        });
        v
    }

    #[test]
    fn identical_manifests_diff_to_empty() {
        let old = sorted(pkg("pkg00"));
        let new = old.clone();
        assert_eq!(SnapshotDiff::compute(&old, &new), SnapshotDiff::default());
        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["pkg00".to_string()]
        );
    }

    #[test]
    fn deep_modified_file_blocks_only_its_containing_chain() {
        let mut old = sorted(vec![
            SnapshotEntry::dir("deep"),
            SnapshotEntry::dir("deep/a"),
            SnapshotEntry::dir("deep/a/b"),
            SnapshotEntry::file("deep/a/b/c", id(9), 0o644),
        ]);
        old.extend(pkg("pkg00"));
        let old = sorted(old);

        let mut new = sorted(vec![
            SnapshotEntry::dir("deep"),
            SnapshotEntry::dir("deep/a"),
            SnapshotEntry::dir("deep/a/b"),
            // Same size/mode, different CONTENT (blob id): modified.
            SnapshotEntry::file("deep/a/b/c", id(8), 0o644),
        ]);
        new.extend(pkg("pkg00"));
        let new = sorted(new);

        let diff = SnapshotDiff::compute(&old, &new);
        assert_eq!(diff.modified.len(), 1);
        assert!(diff.added.is_empty() && diff.deleted.is_empty());
        // deep/* broke, pkg00 survived intact and stays a unit.
        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["pkg00".to_string()]
        );
    }

    #[test]
    fn added_package_dir_leaves_existing_units_intact() {
        let old = sorted(pkg("pkg00"));
        let mut new = sorted(pkg("pkg00"));
        new.extend(pkg("pkg01"));
        let new = sorted(new);

        let diff = SnapshotDiff::compute(&old, &new);
        assert_eq!(diff.added.len(), 3, "one dir + two files");
        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["pkg00".to_string()],
            "the added sibling must not break the existing unit"
        );
    }

    #[test]
    fn deleted_package_dir_is_classified_and_does_not_break_siblings() {
        let mut old = sorted(pkg("pkg00"));
        old.extend(pkg("pkg01"));
        let old = sorted(old);
        let new = sorted(pkg("pkg00"));

        let diff = SnapshotDiff::compute(&old, &new);
        assert_eq!(diff.deleted.len(), 3);
        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["pkg00".to_string()]
        );
    }

    #[test]
    fn mode_flip_blocks_the_containing_unit_but_not_siblings() {
        let mut old = sorted(pkg("pkg00"));
        old.extend(pkg("pkg01"));
        let old = sorted(old);

        let mut new = sorted(vec![
            SnapshotEntry::dir("pkg00"),
            // Exec bit added: normalized mode flips -> modified.
            // Content ids match the healthy originals; ONLY the mode
            // differs.
            SnapshotEntry::file("pkg00/a", id(0), 0o755),
            SnapshotEntry::file("pkg00/b", id(1), 0o644),
        ]);
        new.extend(pkg("pkg01"));
        let new = sorted(new);

        let diff = SnapshotDiff::compute(&old, &new);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["pkg01".to_string()],
            "the flipped package loses its unit, the healthy one keeps it"
        );
    }

    #[test]
    fn root_level_files_are_never_units_and_symlinks_compare_by_target() {
        let old = sorted(vec![
            SnapshotEntry::file("root.txt", id(4), 0o644),
            SnapshotEntry::symlink("ln", "target-a"),
            SnapshotEntry::dir("p"),
            SnapshotEntry::file("p/x", id(5), 0o644),
        ]);
        let new = old.clone();

        // An unchanged root-level file and symlink are simply not units.
        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["p".to_string()]
        );

        // Retargeted symlink: modified, not added+deleted.
        let retarget = sorted(vec![
            SnapshotEntry::file("root.txt", id(4), 0o644),
            SnapshotEntry::symlink("ln", "target-b"),
            SnapshotEntry::dir("p"),
            SnapshotEntry::file("p/x", id(5), 0o644),
        ]);
        let diff = SnapshotDiff::compute(&old, &retarget);
        assert_eq!(diff.modified.len(), 1);
        assert_eq!(diff.modified[0].kind, EntryKind::Symlink);
    }

    #[test]
    fn kind_flip_counts_as_delete_plus_add_and_added_file_breaks_parent_unit() {
        let old = sorted(vec![
            SnapshotEntry::dir("d"),
            SnapshotEntry::file("d/x", id(6), 0o644),
        ]);
        // d/x became a directory holding y.
        let new = sorted(vec![
            SnapshotEntry::dir("d"),
            SnapshotEntry::dir("d/x"),
            SnapshotEntry::file("d/x/y", id(7), 0o644),
        ]);
        let diff = SnapshotDiff::compute(&old, &new);
        assert!(diff.modified.is_empty());
        assert_eq!(diff.deleted.len(), 1);
        assert_eq!(diff.added.len(), 2, "new dir entry + file");
        assert!(
            SnapshotDiff::unchanged_units(&old, &new).is_empty(),
            "nothing survives a kind flip under the only directory"
        );
    }

    #[test]
    fn maximal_units_suppress_fully_unchanged_descendants() {
        // deep/ and deep/inner/ both fully unchanged: only deep/ is
        // reported.
        let old = sorted(vec![
            SnapshotEntry::dir("deep"),
            SnapshotEntry::dir("deep/inner"),
            SnapshotEntry::file("deep/inner/f", id(1), 0o644),
            SnapshotEntry::file("root.txt", id(2), 0o644),
        ]);
        let new = old.clone();
        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["deep".to_string()]
        );
    }
}
