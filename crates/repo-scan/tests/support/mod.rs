//! Shared test harness. Nothing here is a test.
//!
//! Every integration test binary compiles this module independently, so a helper used by only one
//! of them is genuinely unreachable in the others — `discover.rs` never touches the status tree,
//! and `tier0.rs` never touches the discovery tree. That is what the allow is for; it is not
//! covering for anything actually unused.
#![allow(dead_code)]

pub mod fixtures;
