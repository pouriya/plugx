//! The bodies behind the symbols [`export_plugin!`](crate::export_plugin) writes.
//!
//! There is no entry point and nothing is installed. Each symbol is handed the
//! [`Context`](crate::abi::Context) the host built, and each helper here turns that back into a
//! a plugin [`Context`](crate::Context) for exactly the length of the call. The plugin value itself is a `static` in
//! the plugin's own library, which the macro looks after.
//!
//! Everything rests on the contract the ABI documents: the context is fully written, and its
//! `HostApi`, `host_data` and `plugin_name` outlive the process. A host guarantees that by never
//! unloading a plugin library.

use crate::abi::{ABI_VERSION, Context, Slice, Status, ValueApi, ValueHandle, marshal};
use crate::context::Context as PluginContext;
use crate::plugin::{Info, Plugin};
use crate::value::Value;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    /// Why the last call into this plugin returned [`Status::Error`].
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_last_error(message: impl std::fmt::Display) {
    LAST_ERROR.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.clear();
        use std::fmt::Write;
        let _ = write!(slot, "{message}");
    });
}

/// How the plugin reaches the host for one call: its own context, and the host's value vtable.
struct Reached {
    context: PluginContext,
    value_api: &'static ValueApi,
}

/// Turn the host's context into the plugin's own, or report why it cannot be used.
///
/// # Safety
///
/// `context` must be the context a plugx host passed to one of this plugin's symbols: non-null,
/// fully written, with a `HostApi`, `host_data` and `plugin_name` that outlive the process.
unsafe fn reach(context: *const Context) -> Option<Reached> {
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
    // The whole reach, rebuilt per call and kept nowhere: a registration made anywhere under this
    // call — including inside a crate the plugin merely depends on, if it was handed the context —
    // lands in the tables of the host that made the call, and nowhere else.
    // SAFETY: `host` is non-null (checked above), fully written, and valid together with
    // `host_data` for the life of the process.
    let remote = unsafe { crate::remote::Remote::new(context.host, context.host_data) };
    Some(Reached {
        context: PluginContext::remote(name, remote),
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

/// The body of [`plugx_info`](crate::abi::INFO_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed, and `out` a writable location for one owned
/// handle.
pub unsafe fn info(
    context: *const Context,
    out: *mut *mut ValueHandle,
    plugin: fn() -> &'static dyn Plugin,
) -> Status {
    if out.is_null() {
        return Status::Error;
    }
    // SAFETY: forwarded under this function's own contract.
    let reached = match unsafe { reach(context) } {
        Some(reached) => reached,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let info: Info = match plugin().info(&reached.context) {
            Ok(info) => info,
            Err(error) => {
                set_last_error(error);
                return Status::Error;
            }
        };
        // SAFETY: `value_api` is the host's, valid for the process.
        let handle = unsafe { marshal::to_handle(reached.value_api, &info.to_value()) };
        if handle.is_null() {
            set_last_error("the host would not allocate the info tree");
            return Status::Error;
        }
        // SAFETY: `out` was checked non-null above.
        unsafe { *out = handle };
        Status::Ok
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => {
            set_last_error("the plugin panicked while reporting its info");
            Status::Error
        }
    }
}

/// The body of [`plugx_start`](crate::abi::START_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed, and `config` a live handle it owns.
pub unsafe fn start(
    context: *const Context,
    config: *const ValueHandle,
    plugin: fn() -> &'static dyn Plugin,
) -> Status {
    // SAFETY: forwarded under this function's own contract.
    let reached = match unsafe { reach(context) } {
        Some(reached) => reached,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `config` borrows the host's tree for this call.
        let config = match unsafe { read_config(reached.value_api, config) } {
            Some(config) => config,
            None => {
                set_last_error("the configuration could not be read");
                return Status::Error;
            }
        };
        match plugin().start(&reached.context, &config) {
            Ok(()) => Status::Ok,
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => {
            set_last_error("the plugin panicked in start");
            Status::Error
        }
    }
}

/// The body of [`plugx_reload`](crate::abi::RELOAD_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed, and both handles live and owned by it.
pub unsafe fn reload(
    context: *const Context,
    old_config: *const ValueHandle,
    new_config: *const ValueHandle,
    plugin: fn() -> &'static dyn Plugin,
) -> Status {
    // SAFETY: forwarded under this function's own contract.
    let reached = match unsafe { reach(context) } {
        Some(reached) => reached,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: both handles borrow the host's trees for this call.
        let (old, new) = unsafe {
            (
                read_config(reached.value_api, old_config),
                read_config(reached.value_api, new_config),
            )
        };
        let (old, new) = match (old, new) {
            (Some(old), Some(new)) => (old, new),
            _ => {
                set_last_error("the configuration could not be read");
                return Status::Error;
            }
        };
        match plugin().reload(&reached.context, &old, &new) {
            Ok(()) => Status::Ok,
            Err(crate::plugin::Error::Unsupported { .. }) => Status::Unsupported,
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => {
            set_last_error("the plugin panicked in reload");
            Status::Error
        }
    }
}

/// The body of [`plugx_stop`](crate::abi::STOP_SYMBOL).
///
/// # Safety
///
/// `context` must be the context the host passed.
pub unsafe fn stop(context: *const Context, plugin: fn() -> &'static dyn Plugin) -> Status {
    // SAFETY: forwarded under this function's own contract.
    let reached = match unsafe { reach(context) } {
        Some(reached) => reached,
        None => return Status::Incompatible,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| match plugin().stop(&reached.context) {
        Ok(()) => Status::Ok,
        Err(error) => {
            set_last_error(error);
            Status::Error
        }
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => {
            set_last_error("the plugin panicked in stop");
            Status::Error
        }
    }
}

/// The body of [`plugx_last_error`](crate::abi::LAST_ERROR_SYMBOL).
///
/// # Safety
///
/// `out` must be a writable location for one [`Slice`].
pub unsafe fn last_error(out: *mut Slice) -> Status {
    if out.is_null() {
        return Status::Error;
    }
    LAST_ERROR.with(|slot| {
        let slot = slot.borrow();
        // SAFETY: `out` was checked non-null. The slice borrows this thread's error buffer, which
        // the ABI documents as valid only until the next call into this plugin.
        unsafe { *out = Slice::from_str(&slot) };
    });
    Status::Ok
}
