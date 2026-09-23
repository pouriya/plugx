//! # The cdylib ABI
//!
//! The frozen `repr(C)` contract between a plugx host and a plugin loaded from a shared library.
//!
//! Everything here is layout-stable. A change to any struct in this crate means every plugin ever
//! built must be rebuilt, so the rules are strict:
//!
//! - **Structs grow, they never shuffle.** Every vtable starts with a `size: usize` naming its own
//!   byte length. A receiver checks `size` before reading a field the older side may not have
//!   written. Fields are only ever appended.
//! - **Nothing Rust-owned crosses.** Only [`Str`] (pointer + length, which the receiver copies
//!   immediately) and opaque handles freed by whoever created them. A plugin cdylib has its own
//!   allocator; a `String` allocated on one side and freed on the other is undefined behaviour.
//! - **No symbol lookback.** A plugin never resolves symbols in the host. Everything it needs
//!   arrives as function pointers inside [`Context`].
//!
//! A plugin exports one symbol per lifecycle operation — [`VERSION_SYMBOL`], [`INFO_SYMBOL`],
//! [`START_SYMBOL`], [`RELOAD_SYMBOL`], [`STOP_SYMBOL`] — and the host resolves them by name.
//! There is no entry point and no vtable coming back: every call but the version check takes a
//! [`Context`], which is the only thing that leads to the host.
//!
//! Failures travel with the call that failed, in both directions: whoever fails writes a message
//! into the `error_out` it was handed, and the other side copies it out the moment the call
//! returns. Nobody ever calls back to ask.

/// The callback records a host stores on a plugin's behalf.
pub mod callback;
/// The [`Context`] and [`HostApi`] a host hands to a plugin.
pub mod context;
/// Copying a value tree across the boundary in either direction.
pub mod marshal;
/// The lifecycle symbols a plugin exports.
pub mod plugin;
/// Primitive shared types: [`AbiVersion`], [`Str`], [`Status`].
pub mod primitive;
/// Reading and writing a value tree across the boundary by opaque handle.
pub mod value;

pub use crate::abi::AbiVersion;
pub use callback::{ApiCallFn, ApiFunction, Callback, CallbackFn, DropFn};
pub use context::{Context, HostApi};
pub use plugin::{
    INFO_SYMBOL, InfoFn, RELOAD_SYMBOL, ReloadFn, START_SYMBOL, STOP_SYMBOL, StartFn, StopFn,
    VERSION_SYMBOL, VersionFn,
};
pub use primitive::{ABI_VERSION, Status, Str};
pub use value::{ValueApi, ValueHandle};
