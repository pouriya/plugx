//! # Building a cdylib plugin in Rust
//!
//! Two halves of one boundary. [`export`] is what the host calls *in*: the bodies behind the six
//! `plugx_*` symbols [`export_plugin!`](crate::export_plugin) writes. [`host`] is what the plugin
//! calls *out*: the host's `extern "C"` table, behind the same
//! [`Context`](crate::Context) API that code linked into the host uses.
//!
//! Nothing here is reachable without the `compile-cdylib` feature, so a host that only
//! loads plugins never compiles it.

/// The plumbing [`export_plugin!`](crate::export_plugin) generates. Not meant to be called by hand.
pub mod export;
/// Calling the host's vtable from inside a loaded library.
pub(crate) mod host;

pub use crate::abi::cdylib::{ABI_VERSION, AbiVersion, Context as AbiContext};
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
            out: *mut *mut $crate::abi::cdylib::ValueHandle,
        ) -> $crate::abi::cdylib::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::cdylib::export::info(context, out, __plugx_plugin) }
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
            config: *const $crate::abi::cdylib::ValueHandle,
        ) -> $crate::abi::cdylib::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::cdylib::export::start(context, config, __plugx_plugin) }
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
            old_config: *const $crate::abi::cdylib::ValueHandle,
            new_config: *const $crate::abi::cdylib::ValueHandle,
        ) -> $crate::abi::cdylib::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe {
                $crate::sdk::cdylib::export::reload(context, old_config, new_config, __plugx_plugin)
            }
        }

        /// Tears the plugin down. The host has already drained everything it registered.
        ///
        /// # Safety
        ///
        /// `context` must be the context a plugx host passed.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn plugx_stop(
            context: *const $crate::AbiContext,
        ) -> $crate::abi::cdylib::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::cdylib::export::stop(context, __plugx_plugin) }
        }

        /// Why the last call into this plugin, on this thread, failed.
        ///
        /// # Safety
        ///
        /// `out` must be a writable location for one slice.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn plugx_last_error(
            out: *mut $crate::abi::cdylib::Str,
        ) -> $crate::abi::cdylib::Status {
            // SAFETY: forwarded straight from the host, under the same contract this function
            // documents.
            unsafe { $crate::sdk::cdylib::export::last_error(out) }
        }
    };
}
