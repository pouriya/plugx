//! # SDK
//!
//! Write a plugin in Rust, build it as a `cdylib`, and let a plugx host load it.
//!
//! ```rust,ignore
//! use plugx::sdk::prelude::*;
//!
//! #[derive(Default)]
//! struct Redact;
//!
//! impl Plugin for Redact {
//!     fn info(&self, _: &Context) -> Result<Info> {
//!         Ok(Info::new(Version::new(1, 0, 0), "Redacts secrets from log records"))
//!     }
//!
//!     fn start(&self, context: &Context, _config: &Value) -> Result<()> {
//!         context.on_transform("log.record", 0, |_: &Context, data: &mut Value| {
//!             if let Some(map) = data.as_map_mut() {
//!                 map.insert("password", Value::Str("[redacted]".into()));
//!             }
//!             Ok(Flow::Continue)
//!         })?;
//!         context.export("redact", |_: &Context, args: Value| Ok(args))?;
//!         Ok(())
//!     }
//!
//!     fn stop(&self, _: &Context) -> Result<()> { Ok(()) }
//! }
//!
//! plugx::export_plugin!(Redact);
//! ```
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//! ```
//!
//! # What `export_plugin!` does for you
//!
//! It exports one symbol per lifecycle operation — `plugx_abi_version`, `plugx_info`,
//! `plugx_start`, `plugx_reload`, `plugx_stop`, `plugx_last_error` — and keeps your plugin value
//! in a `static` in your own library. There is no entry point, nothing is installed, and nothing
//! is stored on your behalf.
//!
//! Each symbol is handed the host's context and turns it into the [`Context`](crate::Context) your
//! method is called with: a name, and the host's function table. That is the whole mechanism, and
//! it is why there is no setup step to forget. Registering, exporting, calling another plugin and
//! firing a hook all go through that context. If you want one on a background thread, keep a copy
//! — it is `Copy` and `'static`.

/// Everything a plugin author needs, in one `use`.
pub mod prelude {
    pub use crate::context::Context;
    pub use crate::hook::{Flow, Observe, RegistrationId, Transform};
    pub use crate::plugin::{ConfigSpec, Dependency, Error, Info, Plugin, Result, Version};
    pub use crate::value::{Kind, Map, Value};
}

/// The plumbing [`export_plugin!`](crate::export_plugin) generates. Not meant to be called by hand.
pub mod export;

pub use crate::abi::{ABI_VERSION, AbiVersion, Context as AbiContext};
pub use crate::plugin::Plugin;

/// Export a type implementing [`Plugin`] as a loadable plugx plugin.
///
/// With one argument the plugin is built with [`Default::default`]; with two, the second is any
/// expression producing the plugin.
///
/// ```rust,ignore
/// plugx::export_plugin!(Redact);
/// plugx::export_plugin!(Redact, Redact::with_patterns(&["password", "token"]));
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        $crate::export_plugin!($plugin, <$plugin as ::core::default::Default>::default());
    };
    ($plugin:ty, $build:expr) => {
        /// This plugin, built on first use and kept for the life of the process.
        ///
        /// It lives in this library, not in the host: the host holds a name, and reaches the
        /// plugin only by calling the symbols below.
        fn __plugx_plugin() -> &'static dyn $crate::Plugin {
            static PLUGIN: ::std::sync::OnceLock<$plugin> = ::std::sync::OnceLock::new();
            PLUGIN.get_or_init(|| $build)
        }

        /// Reports the plugx ABI this plugin was built against.
        ///
        /// The host calls this first, before anything else in the library: it touches no host
        /// state and allocates nothing, so an incompatible plugin can be discovered and abandoned
        /// safely.
        #[unsafe(no_mangle)]
        pub extern "C" fn plugx_abi_version() -> $crate::AbiVersion {
            $crate::ABI_VERSION
        }

        /// Describes the plugin: version, description, configuration spec, dependencies.
        ///
        /// # Safety
        ///
        /// `context` must be the context a plugx host passed, and `out` a writable location for
        /// one owned value handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn plugx_info(
            context: *const $crate::AbiContext,
            out: *mut *mut $crate::abi::ValueHandle,
        ) -> $crate::abi::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::export::info(context, out, __plugx_plugin) }
        }

        /// Brings the plugin up with its configuration.
        ///
        /// # Safety
        ///
        /// `context` must be the context a plugx host passed, and `config` a live handle the host
        /// owns.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn plugx_start(
            context: *const $crate::AbiContext,
            config: *const $crate::abi::ValueHandle,
        ) -> $crate::abi::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::export::start(context, config, __plugx_plugin) }
        }

        /// Hands the plugin a new configuration.
        ///
        /// # Safety
        ///
        /// `context` must be the context a plugx host passed, and both handles live and owned by
        /// the host.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn plugx_reload(
            context: *const $crate::AbiContext,
            old_config: *const $crate::abi::ValueHandle,
            new_config: *const $crate::abi::ValueHandle,
        ) -> $crate::abi::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::export::reload(context, old_config, new_config, __plugx_plugin) }
        }

        /// Tears the plugin down. The host has already drained everything it registered.
        ///
        /// # Safety
        ///
        /// `context` must be the context a plugx host passed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn plugx_stop(
            context: *const $crate::AbiContext,
        ) -> $crate::abi::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::export::stop(context, __plugx_plugin) }
        }

        /// Why the last call into this plugin, on this thread, failed.
        ///
        /// # Safety
        ///
        /// `out` must be a writable location for one slice.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn plugx_last_error(
            out: *mut $crate::abi::Slice,
        ) -> $crate::abi::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::export::last_error(out) }
        }
    };
}
