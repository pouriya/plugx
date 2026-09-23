use crate::abi::cdylib::primitive::{Str, Status};
use crate::abi::cdylib::value::{ValueApi, ValueHandle};
use crate::value::{Kind, Map, Value};

/// Build a host-owned value tree from a Rust [`Value`], returning an owned handle.
///
/// Returns null if the host refuses an allocation. The caller either releases the handle with
/// [`ValueApi::release`] or hands it to something that takes ownership.
///
/// # Safety
///
/// `api` must be a fully written [`ValueApi`] whose function pointers are valid for this process.
pub unsafe fn to_handle(api: &ValueApi, value: &Value) -> *mut ValueHandle {
    // SAFETY: the caller guarantees every pointer in `api` is callable. Each constructor returns
    // either null or a handle this function owns until it either releases it or transfers it.
    unsafe {
        match value {
            Value::Bool(inner) => (api.new_bool)(*inner),
            Value::Int(inner) => (api.new_int)(*inner),
            Value::Float(inner) => (api.new_float)(*inner),
            Value::Str(inner) => (api.new_str)(Str::from_str(inner)),
            Value::List(item_list) => {
                let handle = (api.new_list)();
                if handle.is_null() {
                    return handle;
                }
                for item in item_list {
                    let child = to_handle(api, item);
                    if child.is_null() {
                        (api.release)(handle);
                        return std::ptr::null_mut();
                    }
                    // `list_push` takes ownership of `child`, so it is not released here.
                    if (api.list_push)(handle, child) != Status::Ok {
                        (api.release)(handle);
                        return std::ptr::null_mut();
                    }
                }
                handle
            }
            Value::Map(map) => {
                let handle = (api.new_map)();
                if handle.is_null() {
                    return handle;
                }
                for (key, item) in map.iter() {
                    let child = to_handle(api, item);
                    if child.is_null() {
                        (api.release)(handle);
                        return std::ptr::null_mut();
                    }
                    // `map_set` takes ownership of `child`.
                    if (api.map_set)(handle, Str::from_str(key), child) != Status::Ok {
                        (api.release)(handle);
                        return std::ptr::null_mut();
                    }
                }
                handle
            }
        }
    }
}

/// Read a host-owned value tree back out into a Rust [`Value`].
///
/// Returns `None` if the handle is null, carries a kind this build does not know, or holds a
/// string that is not UTF-8.
///
/// # Safety
///
/// `api` must be a fully written [`ValueApi`], and `handle` must be a handle it produced that is
/// still valid.
pub unsafe fn from_handle(api: &ValueApi, handle: *const ValueHandle) -> Option<Value> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees `api` is callable and `handle` is live. Every out-parameter
    // below is a stack local, and each is only read after its accessor reported `Status::Ok`.
    unsafe {
        let kind = Kind::from_tag((api.kind)(handle))?;
        match kind {
            Kind::Bool => {
                let mut out = false;
                match (api.get_bool)(handle, &mut out) {
                    Status::Ok => Some(Value::Bool(out)),
                    _ => None,
                }
            }
            Kind::Int => {
                let mut out = 0i64;
                match (api.get_int)(handle, &mut out) {
                    Status::Ok => Some(Value::Int(out)),
                    _ => None,
                }
            }
            Kind::Float => {
                let mut out = 0f64;
                match (api.get_float)(handle, &mut out) {
                    Status::Ok => Some(Value::Float(out)),
                    _ => None,
                }
            }
            Kind::Str => {
                let mut out = Str::EMPTY;
                match (api.get_str)(handle, &mut out) {
                    Status::Ok => {
                        let text = out.to_string_lossless()?;
                        Some(Value::Str(text))
                    }
                    _ => None,
                }
            }
            Kind::List => {
                let mut len = 0usize;
                if (api.list_len)(handle, &mut len) != Status::Ok {
                    return None;
                }
                let mut item_list = Vec::with_capacity(len);
                for index in 0..len {
                    let child = (api.list_get)(handle.cast_mut(), index);
                    match from_handle(api, child) {
                        Some(item) => item_list.push(item),
                        None => return None,
                    }
                }
                Some(Value::List(item_list))
            }
            Kind::Map => {
                let mut len = 0usize;
                if (api.map_len)(handle, &mut len) != Status::Ok {
                    return None;
                }
                let mut map = Map::with_capacity(len);
                for index in 0..len {
                    let mut key = Str::EMPTY;
                    if (api.map_key_at)(handle, index, &mut key) != Status::Ok {
                        return None;
                    }
                    let name = key.to_string_lossless()?;
                    let child = (api.map_get)(handle.cast_mut(), key);
                    match from_handle(api, child) {
                        Some(item) => {
                            map.insert(name, item);
                        }
                        None => return None,
                    }
                }
                Some(Value::Map(map))
            }
        }
    }
}

/// Overwrite an existing host-owned value in place so it matches `value`.
///
/// This is how a transform's result gets back across the boundary: the dispatching side owns the
/// payload handle and never gives it up, so the callback rewrites its contents rather than
/// returning a replacement.
///
/// # Safety
///
/// `api` must be a fully written [`ValueApi`], and `handle` must be a live, mutable handle it
/// produced.
pub unsafe fn write_back(api: &ValueApi, handle: *mut ValueHandle, value: &Value) -> Status {
    if handle.is_null() {
        return Status::Error;
    }
    // SAFETY: the caller guarantees `api` is callable and `handle` is live and mutable. Handles
    // built here are transferred to `list_push` / `map_set`, or released on the failure path.
    unsafe {
        match value {
            Value::Bool(inner) => (api.set_bool)(handle, *inner),
            Value::Int(inner) => (api.set_int)(handle, *inner),
            Value::Float(inner) => (api.set_float)(handle, *inner),
            Value::Str(inner) => (api.set_str)(handle, Str::from_str(inner)),
            Value::List(item_list) => {
                let status = (api.set_list)(handle);
                if status != Status::Ok {
                    return status;
                }
                for item in item_list {
                    let child = to_handle(api, item);
                    if child.is_null() {
                        return Status::Error;
                    }
                    let status = (api.list_push)(handle, child);
                    if status != Status::Ok {
                        return status;
                    }
                }
                Status::Ok
            }
            Value::Map(map) => {
                let status = (api.set_map)(handle);
                if status != Status::Ok {
                    return status;
                }
                for (key, item) in map.iter() {
                    let child = to_handle(api, item);
                    if child.is_null() {
                        return Status::Error;
                    }
                    let status = (api.map_set)(handle, Str::from_str(key), child);
                    if status != Status::Ok {
                        return status;
                    }
                }
                Status::Ok
            }
        }
    }
}
