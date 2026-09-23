//! # ABI
//!
//! The wire contracts between a plugx host and the plugins it loads — one submodule per plugin
//! kind, because they share nothing but the idea.
//!
//! [`AbiVersion`] is the exception: every kind needs a three-part version and every kind's rule
//! for accepting one is the same, so the type lives here and each kind supplies its own constant.

#[cfg(any(feature = "runtime-cdylib", feature = "compile-cdylib"))]
/// The frozen `repr(C)` contract for plugins loaded from a shared library.
pub mod cdylib;

use std::fmt::{Display, Formatter, Result as FmtResult};

/// A three-part version, laid out for the wire.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AbiVersion {
    /// Incompatible change. A host loads a plugin only when this matches exactly.
    pub major: u32,
    /// Backwards-compatible addition (an appended vtable field, a new table entry). A host may load a plugin built against a lower minor than its own.
    pub minor: u32,
    /// A fix with no signature change.
    pub patch: u32,
}

impl AbiVersion {
    /// Whether a plugin built against `self` can be loaded by a host speaking `host`.
    pub const fn compatible_with(self, host: Self) -> bool {
        self.major == host.major && self.minor <= host.minor
    }
}

impl Display for AbiVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}
