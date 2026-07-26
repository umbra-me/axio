//! The tools axio ships, and the machinery they need.
//!
//! This is the only crate that walks a filesystem or spawns a process. Unsafe
//! is denied by default and allowed at exactly one call site, where killing a
//! process group requires it.

#![deny(unsafe_code)]

pub mod diff;
pub mod proc;
pub mod schema;
pub mod tools;

pub use axio_core::policy::{Policy, Verdict};
pub use tools::all;
