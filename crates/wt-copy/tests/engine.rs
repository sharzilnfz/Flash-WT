//! Tests for unified CopyEngine.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::{TreeFixture, assert_trees_identical};
use wt_copy::{BatchItem, CopyEngine, Error, Materializer, SourcePolicy, StrategyPolicy};

#[test]
fn copy_engine_default_copies_directory_and_returns_receipt() {
    let fixture = TreeFixture::heavy_tree(10);
    let dest = fixture.src.parent().unwrap().join("dest-engine-default");

    let engine = CopyEngine::default();
    assert_eq!(engine.policy(), StrategyPolicy::Default);

    let receipt = engine
        .copy_dir(&fixture.src, &dest, SourcePolicy::Immutable)
        .expect("copy_dir");

    assert!(!receipt.strategy.is_empty());
    assert!(receipt.files_copied > 0, "must report positive file count");
    assert!(receipt.bytes_copied > 0, "must report positive byte count");

    assert_trees_identical(&fixture.src, &dest);
}

#[test]
fn copy_engine_force_byte_copy_copies_directory() {
    let fixture = TreeFixture::heavy_tree(10);
    let dest = fixture.src.parent().unwrap().join("dest-engine-bytecopy");

    let engine = CopyEngine::new(StrategyPolicy::ForceByteCopy);
    assert_eq!(engine.policy(), StrategyPolicy::ForceByteCopy);

    let receipt = engine
        .copy_dir(&fixture.src, &dest, SourcePolicy::Any)
        .expect("copy_dir");

    assert_eq!(receipt.strategy, "deep-copy");
    assert!(receipt.files_copied > 0);
    assert!(receipt.bytes_copied > 0);

    assert_trees_identical(&fixture.src, &dest);
}

#[test]
fn copy_engine_hardlink_copies_directory_for_immutable_source() {
    let fixture = TreeFixture::heavy_tree(5);
    let dest = fixture.src.parent().unwrap().join("dest-engine-hardlink");

    let engine = CopyEngine::new(StrategyPolicy::Hardlink);
    let receipt = engine
        .copy_dir(&fixture.src, &dest, SourcePolicy::Immutable)
        .expect("copy_dir");

    assert!(receipt.files_copied > 0);
    assert!(receipt.bytes_copied > 0);
    assert_trees_identical(&fixture.src, &dest);
}

#[test]
fn copy_engine_rejects_existing_destination() {
    let fixture = TreeFixture::heavy_tree(3);
    let dest = fixture.src.parent().unwrap().join("dest-exists");
    fs::create_dir(&dest).expect("create_dir");

    let engine = CopyEngine::default();
    let err = engine
        .copy_dir(&fixture.src, &dest, SourcePolicy::Any)
        .expect_err("must reject existing dest");

    assert!(matches!(err, Error::DestinationExists));
}

#[test]
fn materializer_for_paths_materializes_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src.txt");
    fs::write(&src, "materializer test").expect("write src");
    let dest = temp.path().join("dest.txt");

    let materializer = Materializer::for_paths(StrategyPolicy::Default, &src, &dest);
    assert!(!materializer.strategy().is_empty());

    materializer
        .materialize_file(&src, &dest, None)
        .expect("materialize");
    assert_eq!(
        fs::read_to_string(&dest).expect("read"),
        "materializer test"
    );
}

#[test]
fn materialize_batch_matches_sequential_on_content_mode_counters() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let src_dir = temp.path().join("src");
    let batch_root = temp.path().join("batch");
    let seq_root = temp.path().join("seq");
    fs::create_dir_all(&src_dir).expect("mkdir src");

    let materializer = Materializer::select(StrategyPolicy::ForceByteCopy, false, false, false);
    let mut batch_items = Vec::new();
    let mut seq_items = Vec::new();
    let mut expected_bytes = 0u64;
    for i in 0..20 {
        let content = format!("batch file {i}\n");
        expected_bytes += content.len() as u64;
        let src = src_dir.join(format!("blob-{i}.bin"));
        fs::write(&src, &content).expect("write src");
        let mode = match i % 3 {
            0 => None,
            1 => Some(0o644),
            _ => Some(0o755),
        };
        batch_items.push(BatchItem {
            src: src.clone(),
            dest: batch_root
                .join("a")
                .join("b")
                .join(format!("file-{i}.txt")),
            mode,
            size: content.len() as u64,
        });
        seq_items.push(BatchItem {
            src,
            dest: seq_root.join("a").join("b").join(format!("file-{i}.txt")),
            mode,
            size: content.len() as u64,
        });
    }

    let receipt = materializer
        .materialize_batch(&batch_items)
        .expect("batch");
    assert_eq!(receipt.total, 20);
    assert_eq!(receipt.placed, 20);
    assert_eq!(receipt.shared_cow, 0);
    assert_eq!(receipt.repaired, 0);
    assert_eq!(receipt.bytes_shared, 0);
    assert_eq!(receipt.bytes_copied, expected_bytes);

    let mut seq_placed = 0usize;
    let mut seq_shared = 0usize;
    let mut seq_repaired = 0usize;
    let mut seq_bytes_shared = 0u64;
    let mut seq_bytes_copied = 0u64;
    for item in &seq_items {
        let outcome = materializer
            .materialize_file(&item.src, &item.dest, item.mode)
            .expect("sequential");
        seq_placed += 1;
        if outcome.is_shared_cow {
            seq_shared += 1;
            seq_bytes_shared += item.size;
        } else {
            seq_bytes_copied += item.size;
        }
        if outcome.is_mode_repaired {
            seq_repaired += 1;
        }
    }
    assert_eq!(receipt.placed, seq_placed);
    assert_eq!(receipt.shared_cow, seq_shared);
    assert_eq!(receipt.repaired, seq_repaired);
    assert_eq!(receipt.bytes_shared, seq_bytes_shared);
    assert_eq!(receipt.bytes_copied, seq_bytes_copied);

    for i in 0..20 {
        let batch_dest = &batch_items[i].dest;
        let seq_dest = &seq_items[i].dest;
        assert_eq!(
            fs::read(batch_dest).expect("read batch"),
            fs::read(seq_dest).expect("read seq"),
            "content differs for file {i}"
        );
        let batch_mode = fs::metadata(batch_dest)
            .expect("meta batch")
            .permissions()
            .mode();
        let seq_mode = fs::metadata(seq_dest).expect("meta seq").permissions().mode();
        assert_eq!(batch_mode & 0o7777, seq_mode & 0o7777, "mode differs for file {i}");
        if let Some(want) = batch_items[i].mode {
            assert_eq!(batch_mode & 0o7777, want, "mode not applied for file {i}");
        }
    }
}

#[test]
fn materialize_batch_empty_returns_zero_receipt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let materializer = Materializer::select(StrategyPolicy::ForceByteCopy, false, false, false);
    let _ = temp;
    let receipt = materializer.materialize_batch(&[]).expect("empty batch");
    assert_eq!(receipt.total, 0);
    assert_eq!(receipt.placed, 0);
    assert_eq!(receipt.shared_cow, 0);
    assert_eq!(receipt.repaired, 0);
    assert_eq!(receipt.bytes_shared, 0);
    assert_eq!(receipt.bytes_copied, 0);
}

#[test]
fn materialize_batch_short_circuits_on_first_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("good.bin");
    fs::write(&src, "good\n").expect("write src");
    let missing = temp.path().join("does-not-exist.bin");

    let materializer = Materializer::select(StrategyPolicy::ForceByteCopy, false, false, false);
    let items = vec![
        BatchItem {
            src: src.clone(),
            dest: temp.path().join("out-good.txt"),
            mode: None,
            size: 5,
        },
        BatchItem {
            src: missing,
            dest: temp.path().join("out-bad.txt"),
            mode: None,
            size: 0,
        },
    ];
    let err = materializer
        .materialize_batch(&items)
        .expect_err("missing src must fail");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}
