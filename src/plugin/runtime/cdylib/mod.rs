//! # Running a plugin from a shared library
//!
//! One [`Runtime`] and the two halves of the boundary it needs: [`vtable`], the host's own
//! `extern "C"` table as a plugin sees it, and [`wrap`], a plugin's `repr(C)` callbacks wrapped
//! back into ordinary Rust traits.

/// The host's own value and hook vtables, as handed to a plugin.
pub mod vtable;
/// Wrapping a plugin's `repr(C)` callbacks and vtable back into Rust traits.
pub mod wrap;

use self::vtable::HOST_API;
use self::wrap::{FfiPlugin, PluginSymbols};
use crate::abi::cdylib::{
    ABI_VERSION, Context, INFO_SYMBOL, InfoFn, LAST_ERROR_SYMBOL, LastErrorFn, RELOAD_SYMBOL,
    ReloadFn, START_SYMBOL, STOP_SYMBOL, StartFn, StopFn, Str, VERSION_SYMBOL, VersionFn,
};
use crate::plugin::Plugin;
use crate::plugin::load::{Artifact, Content};
use crate::plugin::runtime::Runtime;
use crate::plugin::runtime::error::{Error, Result};
use crate::registry::Registry;
use cfg_if::cfg_if;
use libloading::{Library, Symbol};
use std::ffi::c_void;

/// Runs a plugin from a `.so`, `.dll` or `.dylib` built against the plugx C ABI.
///
/// The build is a fixed sequence, and the order matters:
///
/// 1. Open the library.
/// 2. Call [`VERSION_SYMBOL`] — the only entry that touches nothing else, so an incompatible
///    plugin is discovered before it has been given anything to hold.
/// 3. Refuse it unless its ABI is compatible with this host's.
/// 4. Resolve the lifecycle symbols, which is all the plugin is: there is no entry point to call
///    and nothing comes back.
///
/// # It has to be a file
///
/// `dlopen` and `LoadLibrary` take a path, and there is no portable way to map a shared library
/// out of memory, so an [`Artifact`] carrying [`Content::Bytes`] is refused with
/// [`Error::Unusable`] rather than written to a temporary file behind the operator's back.
#[derive(Debug, Clone, Copy, Default)]
pub struct Cdylib;

/// The one extension a shared library has on the target this host was built for.
///
/// Taken from the compiler rather than written out, because it is the same constant rustc names
/// the artifact with: `so` on Linux, `dylib` on macOS, `dll` on Windows. Claiming all three would
/// mean this runtime takes artifacts it can never open — a `.dll` is not loadable on Linux at
/// any price — and taking one denies it to a runtime that might have known what to do with it.
///
/// Empty on a target that has no shared libraries at all, so this runtime claims nothing there
/// rather than claiming every file with a bare trailing dot.
const EXTENSION_LIST: &[&str] = if std::env::consts::DLL_EXTENSION.is_empty() {
    &[]
} else {
    &[std::env::consts::DLL_EXTENSION]
};

impl Cdylib {
    /// A runtime for shared libraries.
    pub const fn new() -> Self {
        Self
    }
}

impl Runtime for Cdylib {
    fn name(&self) -> &str {
        "cdylib"
    }

    fn extension_list(&self) -> &[&str] {
        EXTENSION_LIST
    }

    fn build(
        &self,
        artifact: Artifact,
        registry: &'static Registry,
    ) -> Result<Vec<Box<dyn Plugin>>> {
        let path = match &artifact.content {
            Content::Path(path) => path.as_str(),
            Content::Bytes(_) => {
                return Err(Error::Unusable {
                    source: artifact.source.into_boxed_str(),
                    reason: "a shared library has to be a file on disk; \
                             there is no portable way to map one out of memory",
                });
            }
        };

        // The runtime parses the name, because the extension is how it knew the artifact was its
        // business and the rest is its own format's convention. The name is the plugin's identity.
        //
        // The prefix comes from the compiler for the same reason the extension does: rustc writes
        // `libauth.so` on Linux and `auth.dll` on Windows, so stripping a literal `lib` there
        // would turn the crate `libxml` into a plugin called `xml`. `DLL_PREFIX` is empty on
        // Windows, and stripping an empty prefix leaves the stem alone.
        let stem = match artifact.name.rsplit_once('.') {
            Some((stem, _)) => stem,
            None => artifact.name.as_str(),
        };
        let name = match stem.strip_prefix(std::env::consts::DLL_PREFIX) {
            Some(rest) => rest,
            None => stem,
        };
        if name.is_empty() {
            return Err(Error::Unnamed {
                source: artifact.source.into_boxed_str(),
            });
        }

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::info_span!(
                    "plugin.load",
                    plugin = name,
                    runtime = "cdylib"
                )
                .entered();
                tracing::debug!(msg = "Opening plugin library", path = path);
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Opening plugin library\" plugin={name} runtime=cdylib path={path}"
                );
            }
        }

        // SAFETY: opening a library runs its initialisers, which is arbitrary code — that is
        // inherent to loading a plugin at all, and is the trust the operator extends by pointing
        // the host at this source.
        let library = match unsafe { Library::new(path) } {
            Ok(library) => library,
            Err(error) => {
                return Err(Error::Open {
                    source: artifact.source.into_boxed_str(),
                    message: error.to_string().into_boxed_str(),
                });
            }
        };
        // Never unloaded: see the module docs. Leaking is what makes every pointer below valid for
        // the life of the process, which is what `FfiPlugin` and the host both rely on.
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
                        source: artifact.source.into_boxed_str(),
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

        // Leaked once, here. Everything this plugin registers is tagged with this exact `&'static
        // str`, so identity is a pointer the registry already owns rather than a string compared
        // over and over — and a plugin running inside a shared library can hold on to it for good.
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());

        // The plugin is handed this on every call, and the ABI lets it borrow the pieces for the
        // life of the process, so it is leaked. `host_data` is the registry this plugin registers
        // into.
        let host_data = std::ptr::from_ref(registry).cast::<c_void>().cast_mut();
        let context: &'static Context = Box::leak(Box::new(Context {
            size: size_of::<Context>(),
            abi: ABI_VERSION,
            host: &HOST_API,
            host_data,
            plugin_name: Str::from_str(name),
        }));

        // SAFETY: every signature below is fixed by the ABI, which was just checked compatible,
        // and the library is leaked, so the resolved pointers stay valid for the life of the
        // process.
        let symbols = unsafe {
            let info: Symbol<'static, InfoFn> = match library.get(INFO_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_info", artifact.source)),
            };
            let start: Symbol<'static, StartFn> = match library.get(START_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_start", artifact.source)),
            };
            let reload: Symbol<'static, ReloadFn> = match library.get(RELOAD_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_reload", artifact.source)),
            };
            let stop: Symbol<'static, StopFn> = match library.get(STOP_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_stop", artifact.source)),
            };
            let last_error: Symbol<'static, LastErrorFn> = match library.get(LAST_ERROR_SYMBOL) {
                Ok(symbol) => symbol,
                Err(_) => return Err(missing("plugx_last_error", artifact.source)),
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
                tracing::info!(msg = "Loaded plugin library", abi = %plugin_abi);
            } else if #[cfg(feature = "logging")] {
                log::info!(
                    "msg=\"Loaded plugin library\" plugin={name} runtime=cdylib abi={plugin_abi}"
                );
            } else {
                let _ = plugin_abi;
            }
        }

        // One library is one plugin. A bundle format would return several here.
        //
        // SAFETY: the symbols and the context both come from a successful load whose ABI was
        // checked compatible, and both are leaked, so they stay valid for the life of the process.
        Ok(vec![Box::new(unsafe {
            FfiPlugin::new(symbols, context, name)
        })])
    }
}

/// A plugin library missing one of the symbols `export_plugin!` writes.
fn missing(symbol: &'static str, source: String) -> Error {
    Error::MissingSymbol {
        symbol,
        source: source.into_boxed_str(),
    }
}
