//! The plugin's side of the C boundary: what runs inside a library the host loaded.
//!
//! This is the one module outside `abi`, `load` and `sdk` that contains `unsafe`.
//!
//! A plugin `.so` links its own copy of this crate, and the slot that copy has for a host's registry
//! is empty. Nothing here reads it: the host's registry is reached only through the pointers the
//! host passes on every call, which the SDK puts inside a [`Context`]. There is no private copy of
//! anything to accidentally register into.
//!
//! Everything below rests on one contract, established once when the SDK builds the context and
//! relied on by every `unsafe` block here: **the `HostApi` and `host_data` the host passed at
//! entry outlive the process.** That holds because a host never unloads a plugin library, so it
//! never has to invalidate them.

use crate::abi::cdylib::{
    ApiFunction, Callback, HostApi, Str, Status, ValueApi, ValueHandle, marshal,
};
use crate::context::{Context, HostOps};
use crate::error::{Error, Result};
use crate::hook::callback::{Flow, Observe, RegistrationId, Transform};
use crate::registry::ApiFn;
use crate::value::Value;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

/// The host a loaded library talks to.
#[derive(Clone, Copy)]
pub(crate) struct HostRef {
    api: *const HostApi,
    data: *mut c_void,
}

// SAFETY: `HostRef` is two pointers the host guarantees are valid for the life of the process (see
// the module contract). Nothing here is ever mutated.
unsafe impl Send for HostRef {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for HostRef {}

impl HostRef {
    /// Adopt the host's vtable.
    ///
    /// # Safety
    ///
    /// `api` must be a non-null, fully written [`HostApi`] that outlives the process, and `data`
    /// must be the opaque pointer that host wants back on every call.
    pub(crate) const unsafe fn new(api: *const HostApi, data: *mut c_void) -> Self {
        Self { api, data }
    }

    fn api(&self) -> &'static HostApi {
        // SAFETY: module contract — `api` is non-null (checked before the `HostRef` was built) and
        // outlives the process.
        unsafe { &*self.api }
    }

    /// The host's value constructors, which is the only allocator a plugin may use for a tree that
    /// crosses back.
    pub(crate) fn value_api(&self) -> &'static ValueApi {
        // SAFETY: module contract, plus `HostApi::value` is documented never-null.
        unsafe { &*self.api().value }
    }

    /// The host's description of why its last call from this thread failed.
    fn last_error(&self) -> Box<str> {
        let mut slice = Str::EMPTY;
        // SAFETY: module contract. `slice` is a stack local, read only after the host reports
        // success; the borrow is copied out before this returns.
        unsafe {
            if (self.api().last_error)(self.data, &mut slice) != Status::Ok {
                return Box::from("");
            }
            match slice.to_string_lossless() {
                Some(message) => message.into_boxed_str(),
                None => Box::from(""),
            }
        }
    }

    /// The shared body of [`plugin_call`](Self::plugin_call) and [`host_call`](Self::host_call):
    /// copy the argument out, call, copy the result or the error back.
    fn call(
        &self,
        target: &str,
        callee: &str,
        entry: unsafe extern "C" fn(
            *mut c_void,
            Str,
            *const ValueHandle,
            *mut *mut ValueHandle,
        ) -> Status,
        args: Value,
    ) -> Result<Value> {
        let api = self.value_api();
        // SAFETY: module contract. `arguments` is owned here and released on every path; `out` is
        // a stack local the host writes an owned handle into, released here too.
        unsafe {
            let arguments = marshal::to_handle(api, &args);
            if arguments.is_null() {
                return Err(Error::Ffi {
                    operation: target.into(),
                    message: "the host would not allocate the arguments".into(),
                });
            }
            let mut out: *mut ValueHandle = std::ptr::null_mut();
            let status = entry(self.data, Str::from_str(target), arguments, &mut out);
            (api.release)(arguments);

            let returned = match out.is_null() {
                true => None,
                false => {
                    let value = marshal::from_handle(api, out);
                    (api.release)(out);
                    value
                }
            };
            match status {
                Status::Ok | Status::Stop => match returned {
                    Some(value) => Ok(value),
                    None => Ok(Value::map()),
                },
                _ => match returned {
                    Some(error) => Err(Error::Failed {
                        plugin: callee.into(),
                        error: Box::new(error),
                    }),
                    None => Err(Error::Ffi {
                        operation: target.into(),
                        message: self.last_error(),
                    }),
                },
            }
        }
    }
}

// SAFETY of every block below: the module contract — the `HostApi` and `host_data` this
// `HostRef` was built from are valid and outlive the process.
impl HostOps for HostRef {
    /// Forward a dispatch to the host.
    ///
    /// The payload is copied into a host-owned tree, dispatched, and copied back. That round trip
    /// is the price of the two sides not sharing an allocator — and it is the same price a
    /// WebAssembly or Starlark plugin pays, so there is one code path for all of them.
    fn run(&self, hook: &str, data: &mut Value) -> Result<Flow> {
        let api = self.value_api();
        // SAFETY: module contract. `handle` is owned here and released on every path below.
        unsafe {
            let handle = marshal::to_handle(api, data);
            if handle.is_null() {
                return Err(Error::Ffi {
                    operation: hook.into(),
                    message: "the host would not allocate the payload".into(),
                });
            }
            let status = (self.api().run)(self.data, Str::from_str(hook), handle);
            let updated = marshal::from_handle(api, handle);
            (api.release)(handle);

            match status {
                Status::Ok | Status::Stop => {
                    if let Some(updated) = updated {
                        *data = updated;
                    }
                    match status {
                        Status::Stop => Ok(Flow::Stop),
                        _ => Ok(Flow::Continue),
                    }
                }
                _ => Err(Error::Ffi {
                    operation: hook.into(),
                    message: self.last_error(),
                }),
            }
        }
    }

    fn register_transform(
        &self,
        context: Context,
        hook: &str,
        priority: i32,
        callback: Arc<dyn Transform>,
    ) -> Result<RegistrationId> {
        let closure = Box::new(TransformClosure {
            callback,
            context,
            api: self.value_api(),
        });
        let record = Callback {
            call: invoke_transform,
            user_data: Box::into_raw(closure).cast::<c_void>(),
            drop: Some(free::<TransformClosure>),
        };
        let mut id = 0u64;
        // SAFETY: module contract. `id` is a stack local, read only after the host reports
        // success; ownership of `record.user_data` passes to the host, which frees it through the
        // record's `drop`.
        let status = unsafe {
            (self.api().register_transform)(
                self.data,
                Str::from_str(context.name()),
                Str::from_str(hook),
                priority,
                record,
                &mut id,
            )
        };
        match status {
            Status::Ok => Ok(RegistrationId::new(id)),
            _ => Err(Error::Ffi {
                operation: hook.into(),
                message: self.last_error(),
            }),
        }
    }

    fn register_observe(
        &self,
        context: Context,
        hook: &str,
        priority: i32,
        callback: Arc<dyn Observe>,
    ) -> Result<RegistrationId> {
        let closure = Box::new(ObserveClosure {
            callback,
            context,
            api: self.value_api(),
        });
        let record = Callback {
            call: invoke_observe,
            user_data: Box::into_raw(closure).cast::<c_void>(),
            drop: Some(free::<ObserveClosure>),
        };
        let mut id = 0u64;
        // SAFETY: see `register_transform`.
        let status = unsafe {
            (self.api().register_observe)(
                self.data,
                Str::from_str(context.name()),
                Str::from_str(hook),
                priority,
                record,
                &mut id,
            )
        };
        match status {
            Status::Ok => Ok(RegistrationId::new(id)),
            _ => Err(Error::Ffi {
                operation: hook.into(),
                message: self.last_error(),
            }),
        }
    }

    fn unregister(&self, owner: &str, registration: RegistrationId) -> bool {
        // SAFETY: module contract. `owner` is borrowed for this call only, which is what `Str`
        // documents.
        let status = unsafe {
            (self.api().unregister)(self.data, Str::from_str(owner), registration.get())
        };
        status == Status::Ok
    }

    fn export(
        &self,
        context: Context,
        name: &str,
        function: Arc<dyn ApiFn>,
    ) -> Result<RegistrationId> {
        let closure = Box::new(FunctionClosure {
            function,
            context,
            api: self.value_api(),
        });
        let record = ApiFunction {
            call: invoke_function,
            user_data: Box::into_raw(closure).cast::<c_void>(),
            drop: Some(free::<FunctionClosure>),
        };
        let mut id = 0u64;
        // SAFETY: module contract. Ownership of `record.user_data` passes to the host, which frees
        // it through the record's `drop` once the function has been drained and quiesced.
        let status = unsafe {
            (self.api().export)(
                self.data,
                Str::from_str(context.name()),
                Str::from_str(name),
                record,
                &mut id,
            )
        };
        match status {
            Status::Ok => Ok(RegistrationId::new(id)),
            _ => Err(Error::Ffi {
                operation: name.into(),
                message: self.last_error(),
            }),
        }
    }

    fn unexport(&self, owner: &str, registration: RegistrationId) -> bool {
        // SAFETY: module contract.
        let status =
            unsafe { (self.api().unexport)(self.data, Str::from_str(owner), registration.get()) };
        status == Status::Ok
    }

    fn plugin_call(&self, target: &str, args: Value) -> Result<Value> {
        let plugin = match target.split_once("::") {
            Some((plugin, _)) => plugin,
            None => {
                return Err(Error::Malformed {
                    target: target.into(),
                });
            }
        };
        // SAFETY: module contract.
        let entry = self.api().plugin_call;
        self.call(target, plugin, entry, args)
    }

    fn host_call(&self, name: &str, args: Value) -> Result<Value> {
        let entry = self.api().host_call;
        self.call(name, "host", entry, args)
    }
}

/// A Rust [`Transform`] plus everything [`invoke_transform`] needs to call it.
struct TransformClosure {
    callback: Arc<dyn Transform>,
    context: Context,
    api: &'static ValueApi,
}

/// A Rust [`Observe`] plus everything [`invoke_observe`] needs to call it.
struct ObserveClosure {
    callback: Arc<dyn Observe>,
    context: Context,
    api: &'static ValueApi,
}

/// A Rust [`ApiFn`] plus everything [`invoke_function`] needs to call it.
struct FunctionClosure {
    function: Arc<dyn ApiFn>,
    context: Context,
    api: &'static ValueApi,
}

/// Frees one of the closures above once the host has drained it out of its table.
///
/// # Safety
///
/// `user_data` must be the pointer a matching `Box::into_raw::<T>` produced, freed exactly once.
unsafe extern "C" fn free<T>(user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }
    // SAFETY: the caller guarantees this pointer came from `Box::into_raw::<T>` and has not been
    // freed. The host calls this at most once per registration.
    drop(unsafe { Box::from_raw(user_data.cast::<T>()) });
}

/// # Safety
///
/// `user_data` must be a live [`TransformClosure`], and `data` a valid mutable handle from its `api`.
unsafe extern "C" fn invoke_transform(user_data: *mut c_void, data: *mut ValueHandle) -> Status {
    if user_data.is_null() {
        return Status::Error;
    }
    // SAFETY: the host passes back the `user_data` from the registration, which is a live
    // `TransformClosure` until its `drop` runs — and the host never dispatches after dropping.
    let closure = unsafe { &*user_data.cast::<TransformClosure>() };
    // A panic unwinding out of an `extern "C"` frame aborts the host. Catch it here and report a
    // plain error instead: one misbehaving plugin should not take the process down.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `data` is the dispatching side's live payload handle, valid for this call.
        let mut value = match unsafe { marshal::from_handle(closure.api, data) } {
            Some(value) => value,
            None => return Status::Error,
        };
        let flow = match closure.callback.call(&closure.context, &mut value) {
            Ok(flow) => flow,
            Err(_) => return Status::Error,
        };
        // SAFETY: as above; the transform may have rewritten `value`, so copy it back.
        let written = unsafe { marshal::write_back(closure.api, data, &value) };
        match written {
            Status::Ok => match flow {
                Flow::Continue => Status::Ok,
                Flow::Stop => Status::Stop,
            },
            other => other,
        }
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => Status::Error,
    }
}

/// # Safety
///
/// `user_data` must be a live [`ObserveClosure`], and `data` a valid handle from its `api`.
unsafe extern "C" fn invoke_observe(user_data: *mut c_void, data: *mut ValueHandle) -> Status {
    if user_data.is_null() {
        return Status::Error;
    }
    // SAFETY: see `invoke_transform`.
    let closure = unsafe { &*user_data.cast::<ObserveClosure>() };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `data` is the dispatching side's live payload handle, valid for this call. An
        // observe callback only reads it, so nothing is written back.
        let value = match unsafe { marshal::from_handle(closure.api, data) } {
            Some(value) => value,
            None => return Status::Error,
        };
        match closure.callback.call(&closure.context, &value) {
            Ok(Flow::Continue) => Status::Ok,
            Ok(Flow::Stop) => Status::Stop,
            Err(_) => Status::Error,
        }
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => Status::Error,
    }
}

/// # Safety
///
/// `user_data` must be a live [`FunctionClosure`], `args` a valid handle from its `api`, and `out` a
/// writable slot for one owned handle.
unsafe extern "C" fn invoke_function(
    user_data: *mut c_void,
    args: *const ValueHandle,
    out: *mut *mut ValueHandle,
) -> Status {
    if user_data.is_null() || out.is_null() {
        return Status::Error;
    }
    // SAFETY: see `invoke_transform`; the host passes back the `user_data` from the export.
    let closure = unsafe { &*user_data.cast::<FunctionClosure>() };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `args` borrows the caller's tree for this call only.
        let arguments = match unsafe { marshal::from_handle(closure.api, args) } {
            Some(arguments) => arguments,
            None => return Status::Error,
        };
        let (value, status) = match closure.function.call(&closure.context, arguments) {
            Ok(value) => (value, Status::Ok),
            // A `Failed` carries the plugin's own free-form error tree; anything else is a
            // framework failure the caller can only read as text.
            Err(Error::Failed { error, .. }) => (*error, Status::Error),
            Err(error) => (Value::Str(error.to_string()), Status::Error),
        };
        // SAFETY: `closure.api` is the host's, valid for the process; ownership of the handle passes
        // to the host through `out`, which was checked non-null above.
        unsafe {
            let handle = marshal::to_handle(closure.api, &value);
            if handle.is_null() {
                return Status::Error;
            }
            *out = handle;
        }
        status
    }));
    match outcome {
        Ok(status) => status,
        Err(_) => Status::Error,
    }
}
