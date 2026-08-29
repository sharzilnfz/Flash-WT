//! Tests for unified CopyEngine.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::{TreeFixture, assert_trees_identical};
use wt_copy::{
    BatchPlacementReceipt, CopyEngine, Error, Materializer, SourcePolicy, StrategyPolicy,
};

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
fn copy_engine_materialize_file_preserves_content_and_normalizes_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src.txt");
    fs::write(&src, "hello materialize world").expect("write src");

    let engine = CopyEngine::default();

    // Materialize with executable mode
    let dest_exec = temp.path().join("dest_exec.sh");
    let outcome = engine
        .materialize_file(&src, &dest_exec, Some(0o755))
        .expect("materialize_file");

    assert!(!outcome.strategy.is_empty());
    assert_eq!(
        fs::read_to_string(&dest_exec).expect("read dest"),
        "hello materialize world"
    );
    let mode = fs::metadata(&dest_exec).expect("meta").permissions().mode() & 0o7777;
    assert_eq!(mode & 0o111, 0o111, "executable bit must be set");

    // Materialize with non-executable mode
    let dest_plain = temp.path().join("dest_plain.txt");
    engine
        .materialize_file(&src, &dest_plain, Some(0o644))
        .expect("materialize_file");

    let mode_plain = fs::metadata(&dest_plain)
        .expect("meta")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode_plain & 0o111, 0, "executable bit must not be set");
}

#[test]
fn copy_engine_materialize_file_creates_parent_directories() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src.txt");
    fs::write(&src, "nested content").expect("write src");

    let engine = CopyEngine::default();
    let dest = temp.path().join("a/b/c/d/dest.txt");

    engine
        .materialize_file(&src, &dest, None)
        .expect("materialize_file");

    assert_eq!(
        fs::read_to_string(&dest).expect("read dest"),
        "nested content"
    );
}

#[test]
fn copy_engine_materialize_files_batch_accounting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src_dir = temp.path().join("sources");
    let dest_dir = temp.path().join("targets");
    fs::create_dir_all(&src_dir).expect("create src_dir");

    let mut items: Vec<(PathBuf, PathBuf, Option<u32>)> = Vec::new();
    for i in 0..5 {
        let src_file = src_dir.join(format!("file_{i}.txt"));
        fs::write(&src_file, format!("content {i}")).expect("write src");
        let dest_file = dest_dir.join(format!("sub/file_{i}.txt"));
        let mode = if i % 2 == 0 { Some(0o755) } else { Some(0o644) };
        items.push((src_file, dest_file, mode));
    }

    let engine = CopyEngine::default();
    let receipt: BatchPlacementReceipt =
        engine.materialize_files(&items).expect("materialize_files");

    assert_eq!(receipt.total_placed, 5);

    for (src, dest, mode) in &items {
        assert_eq!(
            fs::read_to_string(dest).expect("read"),
            fs::read_to_string(src).expect("read")
        );
        if let Some(expected_mode) = mode {
            let actual_mode = fs::metadata(dest).expect("meta").permissions().mode() & 0o7777;
            assert_eq!(actual_mode & 0o111, expected_mode & 0o111);
        }
    }
}

#[test]
fn materializer_for_paths_and_for_directories() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src.txt");
    fs::write(&src, "materializer test").expect("write src");
    let dest = temp.path().join("dest.txt");

    let m1 = Materializer::for_paths(StrategyPolicy::Default, &src, &dest);
    assert!(!m1.strategy().is_empty());

    let m2 = Materializer::for_directories(StrategyPolicy::Default, &src, &dest);
    assert_eq!(m1.strategy(), m2.strategy());

    m1.materialize_file(&src, &dest, None).expect("materialize");
    assert_eq!(
        fs::read_to_string(&dest).expect("read"),
        "materializer test"
    );
}
