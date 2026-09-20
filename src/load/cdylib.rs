use crate::abi::{
    ABI_VERSION, Context, INFO_SYMBOL, InfoFn, LAST_ERROR_SYMBOL, LastErrorFn, RELOAD_SYMBOL,
    ReloadFn, START_SYMBOL, STOP_SYMBOL, Slice, StartFn, StopFn, VERSION_SYMBOL, VersionFn,
};
use crate::load::error::{Error, Result};
use crate::load::ffi::{FfiPlugin, PluginSymbols};
use crate::load::hostapi::HOST_API;
use crate::load::{Loader, Request};
use crate::plugin::Plugin;
use cfg_if::cfg_if;
use libloading::{Library, Symbol};
use std::ffi::c_void;
use std::path::Path;

/// Loads a plugin from a `.so`, `.dll` or `.dylib` built against the plugx C ABI.
///
/// The load is a fixed sequence, and the order matters:
///
/// 1. Open the library.
/// 2. Call [`VERSION_SYMBOL`] — the only entry that touches nothing else, so an incompatible
///    plugin is discovered before it has been given anything to hold.
/// 3. Refuse it unless its ABI is compatible with this host's.
/// 4. Resolve the lifecycle symbols, which is all the plugin is: there is no entry point to call
///    and nothing comes back.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cdylib;

impl Cdylib {
    /// A loader for shared libraries.
    pub const fn new() -> Self {
        Self
    }
}

impl Loader for Cdylib {
    fn name(&self) -> &'static str {
        "cdylib"
    }

    fn extension_list(&self) -> &[&'static str] {
        &["so", "dll", "dylib"]
    }

    fn load(&self, request: &Request<'_>) -> Result<Box<dyn Plugin>> {
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::info_span!(
                    "plugin.load",
                    plugin = request.name,
                    loader = "cdylib"
                )
                .entered();
                tracing::debug!(msg = "Opening plugin library", path = ?request.path);
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Opening plugin library\" plugin={} loader=cdylib path={:?}",
                    request.name,
                    request.path
                );
            }
        }

        // SAFETY: opening a library runs its initialisers, which is arbitrary code — that is
        // inherent to loading a plugin at all, and is the trust the operator extends by pointing
        // the host at this path.
        let library = match unsafe { Library::new(request.path) } {
            Ok(library) => library,
            Err(error) => {
                return Err(Error::Open {
                    path: request.path.to_path_buf(),
                    message: error.to_string().into_boxed_str(),
                });
            }
        };
        // Never unloaded: see the crate docs. Leaking is what makes every pointer below valid for
        // the life of the process, which is what `FfiPlugin` and the registry both rely on.
        let library: &'static Library = Box::leak(Box::new(library));

        // SAFETY: this symbol's signature is fixed by the ABI. It takes no arguments, touches no
        // host state and allocates nothing, so calling it on a library that turns out to be
        // incompatible is safe.
        let plugin_abi = unsafe {
            let version: Symbol<'static, VersionFn> = match library.get(VERSION_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => {
                    return Err(Error::MissingSymbol {
                        symbol: "plugx_abi_version",
                        path: request.path.to_path_buf(),
                    });
                }
            };
            version()
        };

        if !plugin_abi.compatible_with(ABI_VERSION) {
            return Err(Error::IncompatibleAbi {
                plugin: plugin_abi,
                host: ABI_VERSION,
            });
        }

        // The plugin is handed this on every call, and the ABI lets it borrow the pieces for the
        // life of the process, so it is leaked. `host_data` is the tables this plugin registers
        // into.
        let host_data = std::ptr::from_ref(request.tables)
            .cast::<c_void>()
            .cast_mut();
        let context: &'static Context = Box::leak(Box::new(Context {
            size: size_of::<Context>(),
            abi: ABI_VERSION,
            host: &HOST_API,
            host_data,
            plugin_name: Slice::from_str(request.name),
        }));

        // SAFETY: every signature below is fixed by the ABI, which was just checked compatible,
        // and the library is leaked, so the resolved pointers stay valid for the life of the
        // process.
        let symbols = unsafe {
            let info: Symbol<'static, InfoFn> = match library.get(INFO_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_info", request.path)),
            };
            let start: Symbol<'static, StartFn> = match library.get(START_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_start", request.path)),
            };
            let reload: Symbol<'static, ReloadFn> = match library.get(RELOAD_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_reload", request.path)),
            };
            let stop: Symbol<'static, StopFn> = match library.get(STOP_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_stop", request.path)),
            };
            let last_error: Symbol<'static, LastErrorFn> = match library.get(LAST_ERROR_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_last_error", request.path)),
            };
            PluginSymbols {
                info: *info,
                start: *start,
                reload: *reload,
                stop: *stop,
                last_error: *last_error,
            }
        };

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::info!(
                    msg = "Loaded plugin library",
                    abi = %plugin_abi
                );
            } else if #[cfg(feature = "logging")] {
                log::info!(
                    "msg=\"Loaded plugin library\" plugin={} loader=cdylib abi={}",
                    request.name,
                    plugin_abi
                );
            } else {
                let _ = plugin_abi;
            }
        }

        // SAFETY: the symbols and the context both come from a successful load whose ABI was
        // checked compatible, and both are leaked, so they stay valid for the life of the process.
        Ok(Box::new(unsafe { FfiPlugin::new(symbols, context) }))
    }
}

/// A plugin library missing one of the symbols `export_plugin!` writes.
fn missing(symbol: &'static str, path: &Path) -> Error {
    Error::MissingSymbol {
        symbol,
        path: path.to_path_buf(),
    }
}
