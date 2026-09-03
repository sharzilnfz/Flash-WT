use std::collections::{HashMap, HashSet};

use crate::snapshot::SnapshotEntry;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotDiff {
    pub added: Vec<SnapshotEntry>,

    pub modified: Vec<SnapshotEntry>,

    pub deleted: Vec<SnapshotEntry>,
}

impl SnapshotDiff {
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

    #[must_use]
    pub fn changed_count(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }

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

        let mut merged: Vec<&str> = Vec::with_capacity(old.len() + new.len());
        merged.extend(old.iter().map(|e| e.rel.as_str()));
        merged.extend(new.iter().map(|e| e.rel.as_str()));
        merged.sort_unstable();
        merged.dedup();

        let mut dirty_prefix = vec![0usize; merged.len() + 1];
        for (k, rel) in merged.iter().enumerate() {
            dirty_prefix[k + 1] = dirty_prefix[k] + usize::from(!same.contains(rel));
        }

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

        let mut dirs: Vec<&str> = merged
            .iter()
            .copied()
            .filter(|rel| old_dirs.contains(rel) && new_dirs.contains(rel))
            .collect();
        dirs.sort_by_key(|d| std::cmp::Reverse(d.len()));

        let fully: HashMap<&str, bool> = dirs
            .iter()
            .map(|d| {
                let start = merged.partition_point(|rel| rel_before_bound(rel, d, b'/'));
                let end = merged.partition_point(|rel| rel_before_bound(rel, d, b'0'));
                debug_assert!(end >= start);
                let clean_range = end >= start && dirty_prefix[end] - dirty_prefix[start] == 0;
                (*d, clean_range && same.contains(d))
            })
            .collect();

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

fn rel_before_bound(rel: &str, dir: &str, suffix: u8) -> bool {
    let r = rel.as_bytes();
    let d = dir.as_bytes();
    let Some(head) = r.get(..d.len()) else {
        return true;
    };
    if head == d {
        r.len() == d.len() || r[d.len()] < suffix
    } else {
        head < d
    }
}

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
    use crate::ContentId;
    use crate::snapshot::{EntryKind, SnapshotEntry};

    fn id(n: u8) -> ContentId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        bytes[31] = n;
        ContentId(bytes)
    }

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
            SnapshotEntry::file("deep/a/b/c", id(8), 0o644),
        ]);
        new.extend(pkg("pkg00"));
        let new = sorted(new);

        let diff = SnapshotDiff::compute(&old, &new);
        assert_eq!(diff.modified.len(), 1);
        assert!(diff.added.is_empty() && diff.deleted.is_empty());

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

        assert_eq!(
            SnapshotDiff::unchanged_units(&old, &new),
            vec!["p".to_string()]
        );

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
