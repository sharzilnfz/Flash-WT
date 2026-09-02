//! Tests for unified CopyEngine.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::{TreeFixture, assert_trees_identical};
use wt_copy::{CopyEngine, Error, Materializer, SourcePolicy, StrategyPolicy};

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

    materializer.materialize_file(&src, &dest, None).expect("materialize");
    assert_eq!(
        fs::read_to_string(&dest).expect("read"),
        "materializer test"
    );
}
