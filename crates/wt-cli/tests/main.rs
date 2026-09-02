//! Consolidated integration test suite for `wt-cli`.

// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod it;
