//! Wrapping a plugin's `repr(C)` callbacks and vtable back into ordinary Rust traits.
//!
//! Note what does *not* happen on this side: a payload is already a `plugx::value::Value`, and a
//! [`ValueHandle`](crate::abi::cdylib::ValueHandle) is that same value, so handing one to a plugin is a pointer cast. The copying
//! happens inside the plugin's own `invoke_*` entry point, where it has to.

use crate::abi::cdylib::{
    ApiFunction, Callback, Context as AbiContext, InfoFn, LastErrorFn, ReloadFn, Str, StartFn,
    Status, StopFn, ValueHandle,
};
use crate::context::Context;
use crate::hook::{Flow, Observe, Transform};
use crate::plugin::{Error, Info, Plugin, Result};
use crate::registry::ApiFn;
use crate::value::Value;

/// Asks a plugin why its last call failed, when it bothered to say.
fn plugin_message(symbols: &PluginSymbols) -> String {
    let mut slice = Str::EMPTY;
    // SAFETY: the symbols were resolved from a library that stays mapped for the life of the
    // process, and `slice` is a stack local. The borrow is copied out before this returns, which
    // is the lifetime the ABI gives it.
    unsafe {
        if (symbols.last_error)(&mut slice) != Status::Ok {
            return String::new();
        }
        slice.to_string_lossless().unwrap_or_default()
    }
}

/// The lifecycle symbols a loader resolved out of a plugin's library.
#[derive(Clone, Copy)]
pub struct PluginSymbols {
    /// `plugx_info`.
    pub info: InfoFn,
    /// `plugx_start`.
    pub start: StartFn,
    /// `plugx_reload`.
    pub reload: ReloadFn,
    /// `plugx_stop`.
    pub stop: StopFn,
    /// `plugx_last_error`.
    pub last_error: LastErrorFn,
}

/// A plugin's transform callback, as the registry sees it.
pub struct FfiTransform {
    callback: Callback,
}

impl FfiTransform {
    /// Take ownership of `callback`, including its `user_data`.
    pub fn new(callback: Callback) -> Self {
        Self { callback }
    }
}

impl Transform for FfiTransform {
    fn call(&self, context: &Context, data: &mut Value) -> crate::Result<Flow> {
        let handle = std::ptr::from_mut(data).cast::<ValueHandle>();
        // SAFETY: `handle` borrows the payload for exactly this call, which is what `CallbackFn`
        // documents. `user_data` is the pointer the plugin registered and has not been dropped:
        // the registry only drops a callback after draining it and waiting for in-flight dispatches,
        // so no dispatch can reach a dropped one.
        let status = unsafe { (self.callback.call)(self.callback.user_data, handle) };
        match status {
            Status::Ok => Ok(Flow::Continue),
            Status::Stop => Ok(Flow::Stop),
            _ => Err(crate::Error::Ffi {
                operation: context.name().into(),
                message: "the plugin's callback failed".into(),
            }),
        }
    }
}

impl Drop for FfiTransform {
    fn drop(&mut self) {
        if let Some(drop) = self.callback.drop {
            // SAFETY: the registry drops a callback only after draining it and waiting for every
            // in-flight dispatch, and never twice — so this runs exactly once, with the plugin's
            // library still mapped (plugx never unloads one).
            unsafe { drop(self.callback.user_data) };
        }
    }
}

/// A plugin's observe callback, as the registry sees it.
pub struct FfiObserve {
    callback: Callback,
}

impl FfiObserve {
    /// Take ownership of `callback`, including its `user_data`.
    pub fn new(callback: Callback) -> Self {
        Self { callback }
    }
}

impl Observe for FfiObserve {
    fn call(&self, context: &Context, data: &Value) -> crate::Result<Flow> {
        // The ABI has one callback signature, so an observe callback is handed the same pointer a
        // transform would get. The `&Value` here is what makes it read-only: the plugin side wraps
        // it in an `Observe`, which never writes back.
        let handle = std::ptr::from_ref(data).cast::<ValueHandle>().cast_mut();
        // SAFETY: as `FfiTransform::call`, except the callback is registered as an observer and
        // documented not to write through the handle.
        let status = unsafe { (self.callback.call)(self.callback.user_data, handle) };
        match status {
            Status::Ok => Ok(Flow::Continue),
            Status::Stop => Ok(Flow::Stop),
            _ => Err(crate::Error::Ffi {
                operation: context.name().into(),
                message: "the plugin's callback failed".into(),
            }),
        }
    }
}

impl Drop for FfiObserve {
    fn drop(&mut self) {
        if let Some(drop) = self.callback.drop {
            // SAFETY: see `FfiTransform::drop`.
            unsafe { drop(self.callback.user_data) };
        }
    }
}

/// A plugin's exported function, as the function table sees it.
pub struct FfiApi {
    function: ApiFunction,
}

impl FfiApi {
    /// Take ownership of `function`, including its `user_data`.
    pub fn new(function: ApiFunction) -> Self {
        Self { function }
    }
}

impl ApiFn for FfiApi {
    fn call(&self, context: &Context, args: Value) -> crate::Result<Value> {
        let arguments = std::ptr::from_ref(&args).cast::<ValueHandle>();
        let mut out: *mut ValueHandle = std::ptr::null_mut();
        // SAFETY: `arguments` borrows the caller's tree for exactly this call, which is what
        // `ApiCallFn` documents. `user_data` is the pointer the plugin exported and has not been
        // dropped: the registry only drops a function after draining it and waiting for in-flight
        // calls. `out` is a stack local the plugin writes an owned handle into.
        let status = unsafe { (self.function.call)(self.function.user_data, arguments, &mut out) };
        let mut returned = None;
        if !out.is_null() {
            // SAFETY: a non-null `out` is an owned handle the plugin built with this host's
            // `VALUE_API`, so it is a live `Value` and reclaiming the `Box` is the documented
            // transfer of ownership.
            returned = Some(unsafe { *Box::from_raw(out.cast::<Value>()) });
        }
        match status {
            Status::Ok | Status::Stop => match returned {
                Some(value) => Ok(value),
                None => Ok(Value::map()),
            },
            _ => match returned {
                Some(error) => Err(crate::Error::Failed {
                    plugin: context.name().into(),
                    error: Box::new(error),
                }),
                None => Err(crate::Error::Failed {
                    plugin: context.name().into(),
                    error: Box::new(Value::Str("the plugin's function failed".to_string())),
                }),
            },
        }
    }
}

impl Drop for FfiApi {
    fn drop(&mut self) {
        if let Some(drop) = self.function.drop {
            // SAFETY: see `FfiTransform::drop`.
            unsafe { drop(self.function.user_data) };
        }
    }
}

/// A plugin living behind the C ABI, driven through the ordinary [`Plugin`] trait.
pub struct FfiPlugin {
    symbols: PluginSymbols,
    context: &'static AbiContext,
    name: &'static str,
}

impl FfiPlugin {
    /// Adopt the symbols a runtime resolved, under the name it parsed out of the filename.
    ///
    /// # Safety
    ///
    /// `symbols` and `context` must both come from a successful load of a plugin whose ABI was
    /// checked compatible, and must stay valid for the life of the process — which they do,
    /// because plugx never unloads a plugin library.
    pub const unsafe fn new(
        symbols: PluginSymbols,
        context: &'static AbiContext,
        name: &'static str,
    ) -> Self {
        Self {
            symbols,
            context,
            name,
        }
    }
}

// SAFETY: `Context` is `Sync` (see its own impl) and the rest is function pointers. Every call
// below goes through the plugin's own symbols, which the ABI documents as callable from any
// thread.
unsafe impl Send for FfiPlugin {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for FfiPlugin {}

impl Plugin for FfiPlugin {
    fn info(&self, _context: &Context) -> Result<Info> {
        let mut handle: *mut ValueHandle = std::ptr::null_mut();
        // SAFETY: the symbols are live for the process; `handle` is a stack local the plugin
        // writes an owned handle into, which is released below on every path.
        let status = unsafe { (self.symbols.info)(self.context, &mut handle) };
        if status != Status::Ok {
            return Err(Error::plugin(plugin_message(&self.symbols)));
        }
        if handle.is_null() {
            return Err(Error::plugin("plugin reported no info"));
        }
        // SAFETY: `handle` is an owned handle the plugin built with this host's `VALUE_API`, so it
        // is a live `Value` and reclaiming the `Box` is the documented transfer of ownership.
        let value = unsafe { *Box::from_raw(handle.cast::<Value>()) };
        match Info::from_value(&value) {
            // The name is not in the tree and never crosses the ABI: the plugin already has it, in
            // the `plugin_name` of the context this runtime built for it. Stamped here so a host
            // can learn it from `info` alone.
            Some(mut info) => {
                info.name = self.name;
                Ok(info)
            }
            None => Err(Error::plugin("plugin reported malformed info")),
        }
    }

    fn start(&self, _context: &Context, config: &Value) -> Result<()> {
        let handle = std::ptr::from_ref(config).cast::<ValueHandle>();
        // SAFETY: the symbols are live; `handle` borrows the configuration for this call only,
        // which is what the ABI documents.
        let status = unsafe { (self.symbols.start)(self.context, handle) };
        match status {
            Status::Ok => Ok(()),
            Status::Unsupported => Err(Error::Unsupported { operation: "start" }),
            _ => Err(Error::plugin(plugin_message(&self.symbols))),
        }
    }

    fn reload(&self, _context: &Context, old_config: &Value, new_config: &Value) -> Result<()> {
        let old = std::ptr::from_ref(old_config).cast::<ValueHandle>();
        let new = std::ptr::from_ref(new_config).cast::<ValueHandle>();
        // SAFETY: see `start` — both handles borrow for this call only.
        let status = unsafe { (self.symbols.reload)(self.context, old, new) };
        match status {
            Status::Ok => Ok(()),
            Status::Unsupported => Err(Error::Unsupported {
                operation: "reload",
            }),
            _ => Err(Error::plugin(plugin_message(&self.symbols))),
        }
    }

    fn stop(&self, _context: &Context) -> Result<()> {
        // SAFETY: the symbols are live. The host has already drained and quiesced this plugin's
        // callbacks, so nothing can be running inside it while this executes.
        let status = unsafe { (self.symbols.stop)(self.context) };
        match status {
            Status::Ok => Ok(()),
            _ => Err(Error::plugin(plugin_message(&self.symbols))),
        }
    }
}
