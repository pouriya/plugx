//! # Plugins
//!
//! What a plugin is, from both sides.
//!
//! A host uses this crate to drive a plugin through its lifecycle; a plugin author implements
//! [`Plugin`] and lets the SDK wire it to the C ABI. Neither side needs to know how the other was
//! built — the same trait describes a Rust cdylib, a WebAssembly module and a Starlark script.
//!
//! # Lifecycle
//!
//! ```text
//!   info    → what the plugin is, what it needs, what it can be configured with
//!   start   → bring it up with a validated config; register hook callbacks here
//!   reload  → hand it a new config. Same library, same registrations: this is a
//!             *configuration* reload, never a code reload.
//!   stop    → tear down what start brought up
//! ```
//!
//! By the time [`Plugin::stop`] is called the host has already drained this plugin's callbacks
//! *and* its exported functions out of the tables and waited for every in-flight dispatch and call
//! to finish, so nothing can still be running against the state being torn down.
//!
//! # The context is the plugin's whole world
//!
//! Every method here takes a [`Context`]. It is how a plugin registers callbacks, publishes the
//! functions other plugins can call, calls theirs, calls the application's, and fires hooks. A
//! plugin never reaches a process global, because there is not one to reach.

/// [`Error`] and the crate's [`Result`] alias.
pub mod error;
/// What a plugin reports about itself: [`Info`], [`ConfigSpec`], [`Dependency`], [`Version`].
pub mod info;

pub use crate::context::Context;
pub use error::{Error, Result};
pub use info::{ConfigSpec, Dependency, Info, Version};

pub use crate::abi::ABI_VERSION;

/// The ABI this build speaks, reported by every plugin so a host can refuse an incompatible one
/// before calling anything else in its library.
pub const fn plugx_version() -> crate::abi::AbiVersion {
    crate::abi::ABI_VERSION
}

/// A unit of behaviour a host can load, configure, start and stop.
///
/// Implementations are shared across threads for their whole life, so every method takes `&self`.
/// A plugin that needs mutable state keeps it behind its own lock — the host does not serialise
/// callbacks for you.
pub trait Plugin: Send + Sync {
    /// What this plugin is: its version, what it does, how it can be configured, and what it needs
    /// from other plugins.
    ///
    /// Called once, before [`start`](Plugin::start), and again whenever the host re-inspects the
    /// plugin. Must not depend on having been started.
    fn info(&self, context: &Context) -> Result<Info>;

    /// Bring the plugin up.
    ///
    /// `config` has already been validated against the [`ConfigSpec`] from
    /// [`info`](Plugin::info). This is where hook callbacks are registered, via
    /// [`Context::on_transform`] and [`Context::on_observe`], and where functions other plugins
    /// can call are published, via [`Context::export`].
    fn start(&self, context: &Context, config: &crate::value::Value) -> Result<()>;

    /// Take a new configuration.
    ///
    /// The library stays mapped and registrations stay in place unless the plugin changes them
    /// itself. Both the old and the new configuration are supplied so the plugin can act on the
    /// difference rather than rebuilding from scratch.
    ///
    /// The default implementation reports [`Error::Unsupported`], which a host turns into
    /// stop-then-start.
    fn reload(
        &self,
        context: &Context,
        old_config: &crate::value::Value,
        new_config: &crate::value::Value,
    ) -> Result<()> {
        let _ = (context, old_config, new_config);
        Err(Error::Unsupported {
            operation: "reload",
        })
    }

    /// Tear down whatever [`start`](Plugin::start) brought up.
    ///
    /// Callbacks and exported functions are already drained and quiesced by the time this runs, so
    /// it is safe to drop anything they were using.
    fn stop(&self, context: &Context) -> Result<()>;
}
