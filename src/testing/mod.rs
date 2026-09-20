//! # Testing
//!
//! Helpers for testing hook behaviour, especially the concurrent parts.
//!
//! The interesting bugs in a hook registry are all about timing — a dispatch that overlaps a
//! registration, a stop that lands while callbacks are running — so the helpers here are built to
//! make those overlaps happen on purpose rather than by luck.

/// Callbacks that record what happened to them, and callbacks that can be held open.
pub mod callbacks;

pub use callbacks::{Barrier, Recorder};
