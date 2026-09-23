//! # Runtimes: what a plugin is run as
//!
//! A [`Runtime`] is a **format**. It claims a set of file extensions and turns an
//! [`Artifact`] into running [`Plugin`]s. It does not know or care where the artifact came from —
//! that is a [`Loader`](crate::plugin::load::Loader)'s job, and the two axes multiply rather than
//! pair: one runtime serves every transport, and one transport feeds every runtime.
//!
//! # The runtime parses the name, and the plugin reports it
//!
//! A loader reports a filename exactly as it found it. The runtime turns that into the plugin's
//! identity under its own format's rules — a cdylib drops the `lib` prefix and the extension, so
//! `libauth.so` is `auth` on every platform; another format need not. The extension is also how a
//! runtime is chosen in the first place, which is why parsing belongs on this side.
//!
//! The name does not come back out of [`build`](Runtime::build) beside the plugin, because the
//! plugin already carries it: the runtime leaks it, hands it to the plugin, and stamps it into
//! [`Info::name`], so a host asking a plugin what it is learns what to call it in the same breath.
//! **Every runtime must do this**, or its plugins arrive nameless and a host refuses them.
//!
//! # One artifact, several plugins
//!
//! [`Runtime::build`] returns a `Vec`. A `.so` is one plugin, but a bundle — a zip, a directory
//! image — is not, and a runtime for one should not have to lie about it.
//!
//! # What is opened is never closed
//!
//! Every runtime leaks what it opens. That is deliberate, not an oversight: `dlclose` on a library
//! whose thread-locals, `atexit` handlers or spawned threads are still live is a crash nobody can
//! reproduce, and there is no way for a host to know they are not. Stopping a plugin drains its
//! callbacks and drops its state while the code is still mapped; the mapping itself stays for the
//! life of the process. Replacing a plugin's *code* opens the new build at a fresh source and
//! leaves the old one mapped and dead.
//!
//! Leaking is always safe. Unloading is not.

/// [`Error`] and this module's [`Result`] alias.
pub mod error;

/// The [`Cdylib`](cdylib::Cdylib) runtime: a plugin run out of a shared library.
#[cfg(feature = "runtime-cdylib")]
pub mod cdylib;

pub use error::{Error, Result};

use crate::plugin::Plugin;
use crate::plugin::load::Artifact;
use crate::registry::Registry;

/// A format: something an [`Artifact`] can be run as.
pub trait Runtime: Send + Sync {
    /// A short name for this runtime, as it appears in logs: `"cdylib"`, `"wasm"`.
    fn name(&self) -> &str;

    /// The file extensions this runtime claims, without the dot: `["wasm"]`, `["star", "bzl"]`.
    ///
    /// Claim what this build can actually run, not what the format is called everywhere: the
    /// cdylib runtime claims one extension, the host platform's, because a `.so` is not loadable
    /// on Windows and taking it would only deny it to a runtime that could.
    ///
    /// A host gives each artifact to the first runtime claiming the extension on
    /// [`Artifact::name`], and skips — in silence — an artifact none claims, because a place
    /// plugins are fetched from is allowed to hold other things.
    fn extension_list(&self) -> &[&str];

    /// Turn the artifact into plugins.
    ///
    /// Each one must report its own name from [`Info::name`](crate::plugin::Info::name) — see the
    /// module docs.
    ///
    /// `registry` is what those plugins will register into; it is leaked by the host that owns it,
    /// and reached from inside a plugin only as `host_data`. Whatever this opens stays open for
    /// the life of the process — see the module docs.
    fn build(
        &self,
        artifact: Artifact,
        registry: &'static Registry,
    ) -> Result<Vec<Box<dyn Plugin>>>;
}
