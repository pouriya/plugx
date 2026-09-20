//! # Loaders
//!
//! Turning an artifact on disk into something that implements
//! [`Plugin`].
//!
//! Each kind of artifact is one [`Loader`] behind one cargo feature, so a host that only ever
//! loads shared libraries never compiles a WebAssembly engine.
//!
//! # A loaded library is never unloaded
//!
//! Every loader here leaks what it opens. That is deliberate, not an oversight: `dlclose` on a
//! library whose thread-locals, `atexit` handlers or spawned threads are still live is a crash
//! nobody can reproduce, and there is no way for a host to know they are not. Stopping a plugin
//! drains its callbacks and drops its state while the code is still mapped; the mapping itself
//! stays for the life of the process. Replacing a plugin's *code* opens the new build at a fresh
//! path and leaves the old one mapped and dead.
//!
//! Leaking is always safe. Unloading is not.

/// [`Error`] and the crate's [`Result`] alias.
pub mod error;
/// Wrapping a plugin's `repr(C)` callbacks and vtable back into Rust traits.
pub mod ffi;
/// The host's own value and hook vtables, as handed to a plugin.
pub mod hostapi;

/// Loading a plugin from a shared library.
#[cfg(feature = "load-cdylib")]
pub mod cdylib;

pub use error::{Error, Result};

use crate::plugin::Plugin;
use crate::tables::Tables;
use std::path::Path;

/// What a host is asking a loader to load.
#[derive(Clone, Copy)]
pub struct Request<'a> {
    /// The name the host will know this plugin by. It is the plugin's identity: everything it
    /// registers is tagged with it, and other plugins address its functions through it.
    ///
    /// Leaked by the host, once, so that a plugin running inside a shared library can hold on to
    /// it — and so that the host can hand it back out for the life of the process without
    /// allocating.
    pub name: &'static str,
    /// Where the artifact lives.
    pub path: &'a Path,
    /// The tables this plugin registers into. Leaked by the host that owns them, and reached from
    /// inside the plugin only as `host_data`.
    pub tables: &'static Tables,
}

impl<'a> Request<'a> {
    /// Ask for `path`, to be known as `name`, registering into `tables`.
    pub const fn new(name: &'static str, path: &'a Path, tables: &'static Tables) -> Self {
        Self { name, path, tables }
    }
}

/// One way of turning an artifact into a plugin.
pub trait Loader: Send + Sync {
    /// A short name for this loader, as it appears in logs: `"cdylib"`, `"wasm"`.
    fn name(&self) -> &'static str;

    /// The file extensions this loader takes, without the dot: `["so", "dll", "dylib"]`.
    ///
    /// A host scanning a directory asks each loader in turn and gives the file to the first one
    /// that claims its extension.
    fn extension_list(&self) -> &[&'static str];

    /// Load it.
    ///
    /// Whatever the loader opens stays open for the life of the process — see the crate docs.
    fn load(&self, request: &Request<'_>) -> Result<Box<dyn Plugin>>;
}
