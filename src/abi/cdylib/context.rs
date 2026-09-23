use crate::abi::AbiVersion;
use crate::abi::cdylib::callback::{ApiFunction, Callback};
use crate::abi::cdylib::primitive::{Str, Status};
use crate::abi::cdylib::value::{ValueApi, ValueHandle};
use std::ffi::c_void;

/// Everything a plugin can ask of its host, as function pointers.
///
/// Grows by appending; `size` says how much of it the host actually wrote.
///
/// Every entry takes `host_data` as its first argument — the same opaque pointer the host put in
/// [`Context::host_data`], which is how it finds its registry. The vtable itself is shared and
/// stateless; this pointer is what carries the host.
///
/// Registration entries take the plugin's own name as `owner`. The name is the identity in this
/// ABI; there is no separate id.
#[repr(C)]
pub struct HostApi {
    /// `size_of::<HostApi>()` as the host built it. Check before reading an appended field.
    pub size: usize,

    /// How to read and build values. Never null.
    pub value: *const ValueApi,

    /// Register a callback that may mutate the payload. Writes the new registration id to `out_id`.
    pub register_transform: unsafe extern "C" fn(
        host_data: *mut c_void,
        owner: Str,
        hook: Str,
        priority: i32,
        callback: Callback,
        out_id: *mut u64,
    ) -> Status,

    /// Register a callback that may only read the payload. Writes the new id to `out_id`.
    pub register_observe: unsafe extern "C" fn(
        host_data: *mut c_void,
        owner: Str,
        hook: Str,
        priority: i32,
        callback: Callback,
        out_id: *mut u64,
    ) -> Status,

    /// Remove one of this plugin's own registrations. A plugin may not unregister another's.
    pub unregister: unsafe extern "C" fn(host_data: *mut c_void, owner: Str, id: u64) -> Status,

    /// Fire a hook. This is what a plugin's `Context::run` reaches.
    pub run:
        unsafe extern "C" fn(host_data: *mut c_void, hook: Str, data: *mut ValueHandle) -> Status,

    /// Emit a log line through the host's logger, at a `log`-style level (1 = error … 5 = trace).
    /// A plugin has its own linkage and cannot reach the host's global logger any other way.
    pub log: unsafe extern "C" fn(host_data: *mut c_void, level: u8, message: Str),

    /// Borrow the message describing why the last host call from this thread returned
    /// [`Status::Error`]. Valid until the next call from this thread.
    pub last_error: unsafe extern "C" fn(host_data: *mut c_void, out: *mut Str) -> Status,

    /// Publish a function under this plugin's name. Writes the new registration id to `out_id`.
    pub export: unsafe extern "C" fn(
        host_data: *mut c_void,
        owner: Str,
        name: Str,
        function: ApiFunction,
        out_id: *mut u64,
    ) -> Status,

    /// Withdraw one of this plugin's own functions.
    pub unexport: unsafe extern "C" fn(host_data: *mut c_void, owner: Str, id: u64) -> Status,

    /// Call `plugin::function`. On success `out` receives an owned handle; on [`Status::Error`] it
    /// receives the callee's free-form error value, or stays null when the framework itself
    /// refused the call.
    pub plugin_call: unsafe extern "C" fn(
        host_data: *mut c_void,
        target: Str,
        args: *const ValueHandle,
        out: *mut *mut ValueHandle,
    ) -> Status,

    /// Call one of the application's own functions, by flat name. Same `out` contract as
    /// [`plugin_call`](Self::plugin_call).
    pub host_call: unsafe extern "C" fn(
        host_data: *mut c_void,
        name: Str,
        args: *const ValueHandle,
        out: *mut *mut ValueHandle,
    ) -> Status,
}

// SAFETY: `HostApi` is a table of function pointers plus a `*const ValueApi` the host builds once
// and never mutates. Concurrent `&HostApi` reads only read those pointers.
unsafe impl Send for HostApi {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for HostApi {}

/// What the host hands a plugin at entry, and again on every lifecycle call.
///
/// The `Context` itself is borrowed for the duration of the call it is passed to. [`HostApi`],
/// [`host_data`](Self::host_data) and the bytes behind [`plugin_name`](Self::plugin_name),
/// however, outlive the process — the host never unloads a plugin library, so it never has to
/// invalidate them, and the plugin's SDK keeps all three for good.
#[repr(C)]
pub struct Context {
    /// `size_of::<Context>()` as the host built it. Check before reading an appended field.
    pub size: usize,

    /// The ABI the host speaks. A plugin refuses to run if this is not compatible with the
    /// [`crate::ABI_VERSION`] it was built against.
    pub abi: AbiVersion,

    /// The host's function table. Never null.
    pub host: *const HostApi,

    /// Passed back as the first argument of every [`HostApi`] call. Opaque to the plugin, and the
    /// only thing distinguishing one host in this process from another.
    pub host_data: *mut c_void,

    /// The name the host knows this plugin by. It is the plugin's identity: everything it
    /// registers is tagged with it, and other plugins address its functions through it. Valid for
    /// the life of the process.
    pub plugin_name: Str,
}
