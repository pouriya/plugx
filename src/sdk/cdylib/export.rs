//! The bodies behind the symbols [`export_plugin!`](crate::export_plugin) writes.
//!
//! There is no entry point and nothing is installed. Each symbol is handed the
//! [`Context`](crate::abi::cdylib::Context) the host built, and each helper here turns that back into a
//! a plugin [`Context`](crate::Context) for exactly the length of the call. The plugin value itself is a `static` in
//! the plugin's own library, which the macro looks after.
//!
//! Everything rests on the contract the ABI documents: the context is fully written, and its
//! `HostApi`, `host_data` and `plugin_name` outlive the process. A host guarantees that by never
//! unloading a plugin library.

use crate::abi::cdylib::{ABI_VERSION, Context, Status, Str, ValueApi, ValueHandle, marshal};
use crate::context::Context as PluginContext;
use crate::plugin::{Info, Plugin};
use crate::value::Value;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

/// The host that loaded this library, kept once for the life of the process.
///
/// The ABI guarantees a context's `HostApi` and `host_data` outlive the process, and plugx allows
/// one live `Host` per process — so every call into this plugin carries the same two pointers.
/// Storing them once is what lets a [`PluginContext`] hold a `&'static dyn HostOps` without
/// leaking one per call.
static HOST: OnceLock<crate::sdk::cdylib::host::HostRef> = OnceLock::new();

thread_local! {
    /// Where this plugin parks the message it is handing back through an `error_out`.
    ///
    /// The [`Str`] written into the host's slot borrows this buffer, which the ABI documents as
    /// valid only until the next call into this plugin — by which time the host has copied it.
    static PLUGIN_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Park `message` where `error_out` can borrow it, and answer [`Status::Error`].
///
/// # Safety
///
/// `error_out` must be null, or a writable slot for one [`Str`] belonging to the host.
unsafe fn fail(error_out: *mut Str, message: impl std::fmt::Display) -> Status {
    PLUGIN_ERROR.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.clear();
        use std::fmt::Write;
        let _ = write!(slot, "{message}");
        if !error_out.is_null() {
            // SAFETY: checked non-null, and the caller guarantees it is writable. The borrow points
            // at this thread's buffer, which nothing rewrites before the host has copied it out.
            unsafe { *error_out = Str::from_str(&slot) };
        }
    });
    Status::Error
}

/// What one call into this plugin needs: its own context, and the host's value vtable.
struct Entered {
    context: PluginContext,
    value_api: &'static ValueApi,
}

/// Turn the host's context into the plugin's own, or report why it cannot be used.
///
/// # Safety
///
/// `context` must be the context a plugx host passed to one of this plugin's symbols: non-null,
/// fully written, with a `HostApi`, `host_data` and `plugin_name` that outlive the process.
unsafe fn enter(context: *const Context) -> Option<Entered> {
    if context.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees `context` is a live, fully written `Context`.
    let context = unsafe { &*context };
    if context.size < size_of::<Context>() || context.host.is_null() {
        return None;
    }
    if !ABI_VERSION.compatible_with(context.abi) {
        return None;
    }
    // SAFETY: `HostApi::value` is documented never-null, and the host outlives the process.
    let value_api: &'static ValueApi = unsafe { &*(*context.host).value };
    // SAFETY: the ABI documents `plugin_name` as valid for the life of the process, which is the
    // lifetime taken here. Nothing is copied, so this costs nothing per call.
    let name: &'static str = unsafe { context.plugin_name.as_static_str() }?;
    // The whole of it, rebuilt per call and kept nowhere: a registration made anywhere under this
    // call — including inside a crate the plugin merely depends on, if it was handed the context —
    // lands in the registry of the host that made the call, and nowhere else.
    // SAFETY: `host` is non-null (checked above), fully written, and valid together with
    // `host_data` for the life of the process. Every call passes the same pair, so initialising
    // the slot once and handing out `&'static` from it is sound.
    let host = HOST.get_or_init(|| unsafe {
        crate::sdk::cdylib::host::HostRef::new(context.host, context.host_data)
    });
    Some(Entered {
        context: PluginContext::foreign(name, host),
        value_api,
    })
}

/// Copy a host-owned configuration handle into an owned [`Value`].
///
/// # Safety
///
/// `handle` must be a live handle produced by `api`.
unsafe fn read_config(api: &ValueApi, handle: *const ValueHandle) -> Option<Value> {
    if handle.is_null() {
        return Some(Value::map());
    }
    // SAFETY: the caller guarantees the handle is live and produced by `api`.
    unsafe { marshal::from_handle(api, handle) }
}

/// The body of [`plugx_info`](crate::abi::cdylib::INFO_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed, and `out` a writable location for one owned
/// handle.
pub unsafe fn info(
    context: *const Context,
    out: *mut *mut ValueHandle,
    error_out: *mut Str,
    plugin: fn() -> &'static dyn Plugin,
) -> Status {
    if out.is_null() {
        return Status::Error;
    }
    // SAFETY: forwarded under this function's own contract.
    let entered = match unsafe { enter(context) } {
        Some(entered) => entered,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let info: Info = match plugin().info(&entered.context) {
            Ok(info) => info,
            // SAFETY: `error_out` is the host's slot, forwarded under this function's contract.
            Err(error) => return unsafe { fail(error_out, error) },
        };
        // SAFETY: `value_api` is the host's, valid for the process.
        let handle = unsafe { marshal::to_handle(entered.value_api, &info.to_value()) };
        if handle.is_null() {
            // SAFETY: see above.
            return unsafe { fail(error_out, "the host would not allocate the info tree") };
        }
        // SAFETY: `out` was checked non-null above.
        unsafe { *out = handle };
        Status::Ok
    }));
    match outcome {
        Ok(status) => status,
        // SAFETY: see above.
        Err(_) => unsafe { fail(error_out, "the plugin panicked while reporting its info") },
    }
}

/// The body of [`plugx_start`](crate::abi::cdylib::START_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed, and `config` a live handle it owns.
pub unsafe fn start(
    context: *const Context,
    config: *const ValueHandle,
    error_out: *mut Str,
    plugin: fn() -> &'static dyn Plugin,
) -> Status {
    // SAFETY: forwarded under this function's own contract.
    let entered = match unsafe { enter(context) } {
        Some(entered) => entered,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `config` borrows the host's tree for this call.
        let config = match unsafe { read_config(entered.value_api, config) } {
            Some(config) => config,
            // SAFETY: `error_out` is the host's slot, forwarded under this function's contract.
            None => return unsafe { fail(error_out, "the configuration could not be read") },
        };
        match plugin().start(&entered.context, &config) {
            Ok(()) => Status::Ok,
            // SAFETY: see above.
            Err(error) => unsafe { fail(error_out, error) },
        }
    }));
    match outcome {
        Ok(status) => status,
        // SAFETY: see above.
        Err(_) => unsafe { fail(error_out, "the plugin panicked in start") },
    }
}

/// The body of [`plugx_reload`](crate::abi::cdylib::RELOAD_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed, and both handles live and owned by it.
pub unsafe fn reload(
    context: *const Context,
    old_config: *const ValueHandle,
    new_config: *const ValueHandle,
    error_out: *mut Str,
    plugin: fn() -> &'static dyn Plugin,
) -> Status {
    // SAFETY: forwarded under this function's own contract.
    let entered = match unsafe { enter(context) } {
        Some(entered) => entered,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: both handles borrow the host's trees for this call.
        let (old, new) = unsafe {
            (
                read_config(entered.value_api, old_config),
                read_config(entered.value_api, new_config),
            )
        };
        let (old, new) = match (old, new) {
            (Some(old), Some(new)) => (old, new),
            // SAFETY: `error_out` is the host's slot, forwarded under this function's contract.
            _ => return unsafe { fail(error_out, "the configuration could not be read") },
        };
        match plugin().reload(&entered.context, &old, &new) {
            Ok(()) => Status::Ok,
            Err(crate::plugin::Error::Unsupported { .. }) => Status::Unsupported,
            // SAFETY: see above.
            Err(error) => unsafe { fail(error_out, error) },
        }
    }));
    match outcome {
        Ok(status) => status,
        // SAFETY: see above.
        Err(_) => unsafe { fail(error_out, "the plugin panicked in reload") },
    }
}

/// The body of [`plugx_stop`](crate::abi::cdylib::STOP_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed.
pub unsafe fn stop(
    context: *const Context,
    error_out: *mut Str,
    plugin: fn() -> &'static dyn Plugin,
) -> Status {
    // SAFETY: forwarded under this function's own contract.
    let entered = match unsafe { enter(context) } {
        Some(entered) => entered,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| match plugin().stop(&entered.context) {
        Ok(()) => Status::Ok,
        // SAFETY: `error_out` is the host's slot, forwarded under this function's contract.
        Err(error) => unsafe { fail(error_out, error) },
    }));
    match outcome {
        Ok(status) => status,
        // SAFETY: see above.
        Err(_) => unsafe { fail(error_out, "the plugin panicked in stop") },
    }
}
