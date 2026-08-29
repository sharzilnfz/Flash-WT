//! The `.wtinclude` hydration filter (formerly manifest).
//!
//! # Deprecation / Architecture Note
//!
//! This module has been renamed to [`crate::hydration_filter`] to resolve the
//! domain naming collision with snapshot manifests ([`wt_store::Manifest`]).
//!
//! All items are re-exported here for strict backwards compatibility.

pub use crate::hydration_filter::*;
