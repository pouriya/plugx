//! The host's side of the value and hook vtables.
//!
//! A [`ValueHandle`](crate::abi::cdylib::ValueHandle) here is a `plugx::value::Value`. The host owns every one of them, which is the
//! whole reason a plugin never touches the host's allocator: it asks for a value to be built,
//! read or changed, and never holds memory it did not allocate itself.
//!
//! Handles are owned or borrowed, and the two must not be confused:
//!
//! - **Owned** — produced by the `new_*` constructors as `Box::into_raw`. Freed by `release`,
//!   or consumed by `list_push` / `map_set`, which take the `Box` back.
//! - **Borrowed** — produced by `list_get` / `map_get`, pointing into a tree somebody else owns.
//!   Never freed.

use crate::abi::cdylib::{Status, Str, ValueApi, ValueHandle};
use crate::value::{Map, Value};

/// Reborrow a handle as the value it really is.
///
/// # Safety
///
/// `handle` must be non-null and point at a live `Value` this crate produced.
unsafe fn as_value<'a>(handle: *const ValueHandle) -> &'a Value {
    // SAFETY: the caller guarantees `handle` is a live `Value`.
    unsafe { &*handle.cast::<Value>() }
}

/// Reborrow a handle mutably.
///
/// # Safety
///
/// `handle` must be non-null, point at a live `Value` this crate produced, and be the only
/// reference in use for the duration of the call.
unsafe fn as_value_mut<'a>(handle: *mut ValueHandle) -> &'a mut Value {
    // SAFETY: the caller guarantees exclusive access to a live `Value`.
    unsafe { &mut *handle.cast::<Value>() }
}

/// Hand out a freshly allocated value as an owned handle.
fn own(value: Value) -> *mut ValueHandle {
    Box::into_raw(Box::new(value)).cast::<ValueHandle>()
}

unsafe extern "C" fn kind(handle: *const ValueHandle) -> u8 {
    if handle.is_null() {
        return u8::MAX;
    }
    // SAFETY: non-null was just checked; the ABI contract guarantees a live handle.
    unsafe { as_value(handle) }.kind() as u8
}

unsafe extern "C" fn get_bool(handle: *const ValueHandle, out: *mut bool) -> Status {
    if handle.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: both pointers were checked non-null; `out` is the caller's writable local.
    unsafe {
        match as_value(handle).as_bool() {
            Some(inner) => {
                *out = inner;
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn get_int(handle: *const ValueHandle, out: *mut i64) -> Status {
    if handle.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `get_bool`.
    unsafe {
        match as_value(handle).as_int() {
            Some(inner) => {
                *out = inner;
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn get_float(handle: *const ValueHandle, out: *mut f64) -> Status {
    if handle.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `get_bool`.
    unsafe {
        match as_value(handle).as_float() {
            Some(inner) => {
                *out = inner;
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn get_str(handle: *const ValueHandle, out: *mut Str) -> Status {
    if handle.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `get_bool`. The slice borrows the host's string, which the ABI documents as
    // valid only until the current call returns.
    unsafe {
        match as_value(handle).as_str() {
            Some(text) => {
                *out = Str::from_str(text);
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn list_len(handle: *const ValueHandle, out: *mut usize) -> Status {
    if handle.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `get_bool`.
    unsafe {
        match as_value(handle).as_list() {
            Some(item_list) => {
                *out = item_list.len();
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn list_get(handle: *mut ValueHandle, index: usize) -> *mut ValueHandle {
    if handle.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: non-null checked; the returned pointer borrows into the caller's own tree and is
    // documented as never released by the receiver.
    unsafe {
        match as_value_mut(handle).as_list_mut() {
            Some(item_list) => match item_list.get_mut(index) {
                Some(item) => (item as *mut Value).cast::<ValueHandle>(),
                None => std::ptr::null_mut(),
            },
            None => std::ptr::null_mut(),
        }
    }
}

unsafe extern "C" fn list_push(handle: *mut ValueHandle, item: *mut ValueHandle) -> Status {
    if handle.is_null() || item.is_null() {
        return Status::Error;
    }
    // SAFETY: `item` is an owned handle from a `new_*` constructor, so taking the `Box` back is
    // exactly the transfer of ownership the ABI documents.
    unsafe {
        let item = Box::from_raw(item.cast::<Value>());
        match as_value_mut(handle).as_list_mut() {
            Some(item_list) => {
                item_list.push(*item);
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn map_len(handle: *const ValueHandle, out: *mut usize) -> Status {
    if handle.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `get_bool`.
    unsafe {
        match as_value(handle).as_map() {
            Some(map) => {
                *out = map.len();
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn map_key_at(handle: *const ValueHandle, index: usize, out: *mut Str) -> Status {
    if handle.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `get_str` — the key slice borrows the host's string for this call only.
    unsafe {
        match as_value(handle).as_map() {
            Some(map) => match map.entry_at(index) {
                Some((key, _)) => {
                    *out = Str::from_str(key);
                    Status::Ok
                }
                None => Status::Error,
            },
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn map_get(handle: *mut ValueHandle, key: Str) -> *mut ValueHandle {
    if handle.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `key` is valid for this call by the ABI contract; the returned pointer borrows into
    // the caller's own tree and is never released by the receiver.
    unsafe {
        let key = match key.to_string_lossless() {
            Some(key) => key,
            None => return std::ptr::null_mut(),
        };
        match as_value_mut(handle).as_map_mut() {
            Some(map) => match map.get_mut(&key) {
                Some(item) => (item as *mut Value).cast::<ValueHandle>(),
                None => std::ptr::null_mut(),
            },
            None => std::ptr::null_mut(),
        }
    }
}

unsafe extern "C" fn map_set(handle: *mut ValueHandle, key: Str, item: *mut ValueHandle) -> Status {
    if handle.is_null() || item.is_null() {
        return Status::Error;
    }
    // SAFETY: `item` is an owned handle, reclaimed here; `key` is valid for this call.
    unsafe {
        let item = Box::from_raw(item.cast::<Value>());
        let key = match key.to_string_lossless() {
            Some(key) => key,
            None => return Status::Error,
        };
        match as_value_mut(handle).as_map_mut() {
            Some(map) => {
                map.insert(key, *item);
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn map_remove(handle: *mut ValueHandle, key: Str) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: `key` is valid for this call by the ABI contract.
    unsafe {
        let key = match key.to_string_lossless() {
            Some(key) => key,
            None => return Status::Error,
        };
        match as_value_mut(handle).as_map_mut() {
            Some(map) => match map.remove(&key) {
                Some(_) => Status::Ok,
                None => Status::Error,
            },
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn set_bool(handle: *mut ValueHandle, item: bool) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: exclusive access to a live handle, per the ABI contract.
    unsafe { *as_value_mut(handle) = Value::Bool(item) };
    Status::Ok
}

unsafe extern "C" fn set_int(handle: *mut ValueHandle, item: i64) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: see `set_bool`.
    unsafe { *as_value_mut(handle) = Value::Int(item) };
    Status::Ok
}

unsafe extern "C" fn set_float(handle: *mut ValueHandle, item: f64) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: see `set_bool`.
    unsafe { *as_value_mut(handle) = Value::Float(item) };
    Status::Ok
}

unsafe extern "C" fn set_str(handle: *mut ValueHandle, item: Str) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: see `set_bool`; `item` is valid for this call and copied before it returns.
    unsafe {
        match item.to_string_lossless() {
            Some(text) => {
                *as_value_mut(handle) = Value::Str(text);
                Status::Ok
            }
            None => Status::Error,
        }
    }
}

unsafe extern "C" fn set_list(handle: *mut ValueHandle) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: see `set_bool`.
    unsafe { *as_value_mut(handle) = Value::List(Vec::new()) };
    Status::Ok
}

unsafe extern "C" fn set_map(handle: *mut ValueHandle) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: see `set_bool`.
    unsafe { *as_value_mut(handle) = Value::Map(Map::new()) };
    Status::Ok
}

unsafe extern "C" fn new_bool(item: bool) -> *mut ValueHandle {
    own(Value::Bool(item))
}

unsafe extern "C" fn new_int(item: i64) -> *mut ValueHandle {
    own(Value::Int(item))
}

unsafe extern "C" fn new_float(item: f64) -> *mut ValueHandle {
    own(Value::Float(item))
}

unsafe extern "C" fn new_str(item: Str) -> *mut ValueHandle {
    // SAFETY: `item` is valid for this call by the ABI contract, and copied before returning.
    match unsafe { item.to_string_lossless() } {
        Some(text) => own(Value::Str(text)),
        None => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn new_list() -> *mut ValueHandle {
    own(Value::List(Vec::new()))
}

unsafe extern "C" fn new_map() -> *mut ValueHandle {
    own(Value::Map(Map::new()))
}

unsafe extern "C" fn release(handle: *mut ValueHandle) {
    if handle.is_null() {
        return;
    }
    // SAFETY: the ABI documents `release` as taking an owned handle from a `new_*` constructor,
    // exactly once. Borrowed handles from `list_get` / `map_get` are documented as never released.
    drop(unsafe { Box::from_raw(handle.cast::<Value>()) });
}

/// The value vtable this host hands to every plugin it loads.
pub static VALUE_API: ValueApi = ValueApi {
    size: size_of::<ValueApi>(),
    kind,
    get_bool,
    get_int,
    get_float,
    get_str,
    list_len,
    list_get,
    list_push,
    map_len,
    map_key_at,
    map_get,
    map_set,
    map_remove,
    set_bool,
    set_int,
    set_float,
    set_str,
    set_list,
    set_map,
    new_bool,
    new_int,
    new_float,
    new_str,
    new_list,
    new_map,
    release,
};

use super::wrap::{FfiApi, FfiObserve, FfiTransform};
use crate::abi::cdylib::{ApiFunction, Callback, HostApi};
use crate::context::{Context, HostAccess};
use crate::registry::Registry;
use std::cell::RefCell;
use std::ffi::c_void;

thread_local! {
    /// The message behind the last [`Status::Error`] this thread returned to a plugin.
    ///
    /// Per-thread because the host dispatches on whatever thread the plugin called from, and a
    /// shared slot would let one thread's failure overwrite another's before it was read.
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };

    /// What the callback now running on this thread recorded on its way out, through
    /// [`continue_with_error`] or [`stop_with_error`].
    ///
    /// Written from inside the callback and taken by the dispatching side the instant the callback
    /// returns, on the same thread, with nothing in between — so a nested dispatch started by that
    /// callback has finished and taken its own message long before this one is read.
    static CALLBACK_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

fn set_last_error(message: impl std::fmt::Display) {
    LAST_ERROR.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.clear();
        use std::fmt::Write;
        let _ = write!(slot, "{message}");
    });
}

/// Take whatever the callback that just returned recorded, and empty the slot.
///
/// Empty when the callback reported a failing status without saying anything, which a C plugin can
/// do by returning the bare code instead of going through one of the two helpers.
pub(crate) fn take_callback_error() -> String {
    CALLBACK_ERROR.with(|slot| std::mem::take(&mut *slot.borrow_mut()))
}

unsafe extern "C" fn continue_with_error(_host_data: *mut c_void, message: Str) -> Status {
    // SAFETY: `message` is valid for this call by the ABI contract, and copied before returning.
    record_callback_error(unsafe { message.to_string_lossless() });
    Status::ContinueError
}

unsafe extern "C" fn stop_with_error(_host_data: *mut c_void, message: Str) -> Status {
    // SAFETY: see `continue_with_error`.
    record_callback_error(unsafe { message.to_string_lossless() });
    Status::Error
}

/// Put a callback's own message where the dispatching side will pick it up.
fn record_callback_error(message: Option<String>) {
    let message = match message {
        Some(message) => message,
        None => "the plugin's error message was not valid UTF-8".to_string(),
    };
    CALLBACK_ERROR.with(|slot| *slot.borrow_mut() = message);
}

/// Recover the registry a plugin was loaded into.
///
/// This is what `host_data` is for: the loader put a pointer to its host's [`Registry`] there, so a
/// plugin reaches them without the vtable having to hold any state of its own.
///
/// # Safety
///
/// `host_data` must be the pointer this crate's cdylib loader wrote into the plugin's context: a
/// leaked `Registry` valid for the life of the process.
unsafe fn registry<'a>(host_data: *mut c_void) -> Option<&'a Registry> {
    if host_data.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees this is the leaked `Registry` the loader wrote.
    Some(unsafe { &*host_data.cast::<Registry>() })
}

unsafe extern "C" fn register_transform(
    host_data: *mut c_void,
    owner: Str,
    hook: Str,
    priority: i32,
    callback: Callback,
    out_id: *mut u64,
) -> Status {
    if out_id.is_null() {
        return Status::Error;
    }
    // SAFETY: `host_data` is the loader's leaked `Registry`; `owner` and `hook` are valid for this
    // call by the ABI contract and copied here; `out_id` was checked non-null and is the caller's
    // writable local. Ownership of `callback.user_data` passes to `FfiTransform`, which frees it
    // through the record's `drop` when the registry drop it.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => {
                set_last_error("the plugin was given no registry to register into");
                return Status::Error;
            }
        };
        let (owner, hook) = match (owner.to_string_lossless(), hook.to_string_lossless()) {
            (Some(owner), Some(hook)) => (owner, hook),
            _ => {
                set_last_error("owner or hook name was not valid UTF-8");
                return Status::Error;
            }
        };
        let owner = match registry.known_name(&owner) {
            Some(owner) => owner,
            None => {
                set_last_error("no plugin of that name is loaded here");
                return Status::Error;
            }
        };
        let id = Context::direct(owner, registry).on_transform(
            hook.as_str(),
            priority,
            FfiTransform::new(callback),
        );
        match id {
            Ok(id) => {
                *out_id = id.get();
                Status::Ok
            }
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }
}

unsafe extern "C" fn register_observe(
    host_data: *mut c_void,
    owner: Str,
    hook: Str,
    priority: i32,
    callback: Callback,
    out_id: *mut u64,
) -> Status {
    if out_id.is_null() {
        return Status::Error;
    }
    // SAFETY: see `register_transform`.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => {
                set_last_error("the plugin was given no registry to register into");
                return Status::Error;
            }
        };
        let (owner, hook) = match (owner.to_string_lossless(), hook.to_string_lossless()) {
            (Some(owner), Some(hook)) => (owner, hook),
            _ => {
                set_last_error("owner or hook name was not valid UTF-8");
                return Status::Error;
            }
        };
        let owner = match registry.known_name(&owner) {
            Some(owner) => owner,
            None => {
                set_last_error("no plugin of that name is loaded here");
                return Status::Error;
            }
        };
        let id = Context::direct(owner, registry).on_observe(
            hook.as_str(),
            priority,
            FfiObserve::new(callback),
        );
        match id {
            Ok(id) => {
                *out_id = id.get();
                Status::Ok
            }
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }
}

unsafe extern "C" fn unregister(host_data: *mut c_void, owner: Str, id: u64) -> Status {
    // SAFETY: `host_data` is the loader's leaked `Registry`; `owner` is valid for this call.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => return Status::Error,
        };
        let owner = match owner.to_string_lossless() {
            Some(owner) => owner,
            None => return Status::Error,
        };
        let owner = match registry.known_name(&owner) {
            Some(owner) => owner,
            None => return Status::Error,
        };
        match Context::direct(owner, registry).unregister(crate::RegistrationId::new(id)) {
            true => Status::Ok,
            false => {
                set_last_error("no such registration for this plugin");
                Status::Error
            }
        }
    }
}

unsafe extern "C" fn run(host_data: *mut c_void, hook: Str, data: *mut ValueHandle) -> Status {
    if data.is_null() {
        return Status::Error;
    }
    // SAFETY: `host_data` is the loader's leaked `Registry`; `hook` is valid for this call; `data` is
    // an owned handle the plugin allocated through `VALUE_API`, so it is a live `Value` and the
    // plugin is not touching it concurrently.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => {
                set_last_error("the plugin was given no registry to dispatch into");
                return Status::Error;
            }
        };
        let hook = match hook.to_string_lossless() {
            Some(hook) => hook,
            None => {
                set_last_error("hook name was not valid UTF-8");
                return Status::Error;
            }
        };
        match registry.dispatch(
            HostAccess::Direct(registry),
            hook.as_str().into(),
            as_value_mut(data),
        ) {
            Ok(()) => Status::Ok,
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }
}

unsafe extern "C" fn log(_host_data: *mut c_void, level: u8, message: Str) {
    // SAFETY: `message` is valid for this call by the ABI contract, and copied before returning.
    let text = match unsafe { message.to_string_lossless() } {
        Some(text) => text,
        None => return,
    };
    cfg_if::cfg_if! {
        if #[cfg(feature = "tracing")] {
            match level {
                1 | 2 => tracing::warn!(msg = "Plugin reported", detail = %text),
                3 => tracing::info!(msg = "Plugin reported", detail = %text),
                4 => tracing::debug!(msg = "Plugin reported", detail = %text),
                _ => tracing::trace!(msg = "Plugin reported", detail = %text),
            }
        } else if #[cfg(feature = "logging")] {
            match level {
                1 | 2 => log::warn!("msg=\"Plugin reported\" detail={text:?}"),
                3 => log::info!("msg=\"Plugin reported\" detail={text:?}"),
                4 => log::debug!("msg=\"Plugin reported\" detail={text:?}"),
                _ => log::trace!("msg=\"Plugin reported\" detail={text:?}"),
            }
        } else {
            let _ = (level, text);
        }
    }
}

unsafe extern "C" fn last_error(_host_data: *mut c_void, out: *mut Str) -> Status {
    if out.is_null() {
        return Status::Error;
    }
    LAST_ERROR.with(|slot| {
        let slot = slot.borrow();
        // SAFETY: `out` was checked non-null. The slice borrows this thread's error buffer, which
        // the ABI documents as valid only until this thread's next host call.
        unsafe { *out = Str::from_str(&slot) };
    });
    Status::Ok
}

unsafe extern "C" fn export(
    host_data: *mut c_void,
    owner: Str,
    name: Str,
    function: ApiFunction,
    out_id: *mut u64,
) -> Status {
    if out_id.is_null() {
        return Status::Error;
    }
    // SAFETY: as `register_transform` — ownership of `function.user_data` passes to `FfiApi`.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => {
                set_last_error("the plugin was given no registry to export into");
                return Status::Error;
            }
        };
        let (owner, name) = match (owner.to_string_lossless(), name.to_string_lossless()) {
            (Some(owner), Some(name)) => (owner, name),
            _ => {
                set_last_error("owner or function name was not valid UTF-8");
                return Status::Error;
            }
        };
        let owner = match registry.known_name(&owner) {
            Some(owner) => owner,
            None => {
                set_last_error("no plugin of that name is loaded here");
                return Status::Error;
            }
        };
        match Context::direct(owner, registry).export(name.as_str(), FfiApi::new(function)) {
            Ok(id) => {
                *out_id = id.get();
                Status::Ok
            }
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }
}

unsafe extern "C" fn unexport(host_data: *mut c_void, owner: Str, id: u64) -> Status {
    // SAFETY: see `unregister`.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => return Status::Error,
        };
        let owner = match owner.to_string_lossless() {
            Some(owner) => owner,
            None => return Status::Error,
        };
        let owner = match registry.known_name(&owner) {
            Some(owner) => owner,
            None => return Status::Error,
        };
        match Context::direct(owner, registry).unexport(crate::RegistrationId::new(id)) {
            true => Status::Ok,
            false => {
                set_last_error("no such export for this plugin");
                Status::Error
            }
        }
    }
}

unsafe extern "C" fn plugin_call(
    host_data: *mut c_void,
    target: Str,
    args: *const ValueHandle,
    out: *mut *mut ValueHandle,
) -> Status {
    if out.is_null() {
        return Status::Error;
    }
    // SAFETY: `host_data` is the loader's leaked `Registry`; `target` is valid for this call; `args`
    // is an owned handle the plugin allocated through `VALUE_API`, borrowed and copied here; `out`
    // was checked non-null and receives a handle the host owns until the plugin releases it.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => {
                set_last_error("the plugin was given no registry to call into");
                return Status::Error;
            }
        };
        let target = match target.to_string_lossless() {
            Some(target) => target,
            None => {
                set_last_error("target name was not valid UTF-8");
                return Status::Error;
            }
        };
        let arguments = match args.is_null() {
            true => Value::map(),
            false => as_value(args).clone(),
        };
        match registry.plugin_call(HostAccess::Direct(registry), target.as_str(), arguments) {
            Ok(value) => {
                *out = own(value);
                Status::Ok
            }
            Err(crate::Error::Failed { error, .. }) => {
                *out = own(*error);
                Status::Error
            }
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }
}

unsafe extern "C" fn host_call(
    host_data: *mut c_void,
    name: Str,
    args: *const ValueHandle,
    out: *mut *mut ValueHandle,
) -> Status {
    if out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `plugin_call`.
    unsafe {
        let registry = match registry(host_data) {
            Some(registry) => registry,
            None => {
                set_last_error("the plugin was given no registry to call into");
                return Status::Error;
            }
        };
        let name = match name.to_string_lossless() {
            Some(name) => name,
            None => {
                set_last_error("function name was not valid UTF-8");
                return Status::Error;
            }
        };
        let arguments = match args.is_null() {
            true => Value::map(),
            false => as_value(args).clone(),
        };
        match registry.host_call(HostAccess::Direct(registry), name.as_str(), arguments) {
            Ok(value) => {
                *out = own(value);
                Status::Ok
            }
            Err(crate::Error::Failed { error, .. }) => {
                *out = own(*error);
                Status::Error
            }
            Err(error) => {
                set_last_error(error);
                Status::Error
            }
        }
    }
}

/// The host vtable handed to every plugin this crate loads.
pub static HOST_API: HostApi = HostApi {
    size: size_of::<HostApi>(),
    value: &VALUE_API,
    register_transform,
    register_observe,
    unregister,
    run,
    log,
    continue_with_error,
    stop_with_error,
    last_error,
    export,
    unexport,
    plugin_call,
    host_call,
};
