use crate::abi::AbiVersion;
use crate::abi::cdylib::callback::{ApiFunction, Callback};
use crate::abi::cdylib::primitive::{Status, Str};
use crate::abi::cdylib::value::{ValueApi, ValueHandle};
use std::ffi::c_void;

/// Everything a plugin can ask of its host, as function pointers.
///
/// Grows by appending; `size` says how much of it the host actually wrote.
///
/// Every entry takes `host_data` as its first argument — the same opaque pointer the host put in
/// [`Context::host_data`]. The vtable itself is shared and stateless; this pointer is what carries
/// the host, and it is **per plugin**: one host hands a different one to each library it loads.
///
/// That is why nothing here names an owner. A registration is tagged with whoever `host_data` says
/// is calling, which the plugin cannot spell wrong and cannot spell as somebody else. The name is
/// still the identity in this ABI and there is still no separate id — the plugin just does not
/// have to repeat it.
///
/// # Reporting a failure
///
/// Every entry that can fail takes an `error_out`, and writes a message into it before returning
/// anything but [`Status::Ok`]. Copy it immediately: it borrows a buffer the host reuses on this
/// thread's next call. `error_out` may be null, and a host that has nothing to say leaves it
/// alone, so initialise it to [`Str::EMPTY`] and treat empty as "no detail".
///
/// [`unregister`](Self::unregister) and [`unexport`](Self::unexport) have none, because their
/// whole answer is whether the registration was there.
#[repr(C)]
pub struct HostApi {
    /// `size_of::<HostApi>()` as the host built it. Check before reading an appended field.
    pub size: usize,

    /// How to read and build values. Never null.
    pub value: *const ValueApi,

    /// Register a callback that may mutate the payload. Writes the new registration id to `out_id`.
    pub register_transform: unsafe extern "C" fn(
        host_data: *mut c_void,
        hook: Str,
        priority: i32,
        callback: Callback,
        out_id: *mut u64,
        error_out: *mut Str,
    ) -> Status,

    /// Register a callback that may only read the payload. Writes the new id to `out_id`.
    pub register_observe: unsafe extern "C" fn(
        host_data: *mut c_void,
        hook: Str,
        priority: i32,
        callback: Callback,
        out_id: *mut u64,
        error_out: *mut Str,
    ) -> Status,

    /// Remove one of this plugin's own registrations. A plugin cannot reach another's: the id is
    /// looked up under whoever `host_data` says is calling.
    pub unregister: unsafe extern "C" fn(host_data: *mut c_void, id: u64) -> Status,

    /// Fire a hook. This is what a plugin's `Context::run` reaches.
    pub run: unsafe extern "C" fn(
        host_data: *mut c_void,
        hook: Str,
        data: *mut ValueHandle,
        error_out: *mut Str,
    ) -> Status,

    /// Emit a log line through the host's logger, at a `log`-style level (1 = error … 5 = trace).
    /// A plugin has its own linkage and cannot reach the host's global logger any other way.
    pub log: unsafe extern "C" fn(host_data: *mut c_void, level: u8, message: Str),

    /// From inside a hook callback: record why this callback failed, and let the dispatch carry on
    /// to the next callback anyway.
    ///
    /// Returns [`Status::ContinueError`], which is the code the callback then returns — so the
    /// whole of it is `return host->continue_with_error(host_data, message);`. The host copies the
    /// message before this returns and logs it at `warn` against this plugin; it does not reach
    /// whoever fired the hook.
    ///
    /// Calling this anywhere but on the way out of a callback does nothing useful.
    pub continue_with_error: unsafe extern "C" fn(host_data: *mut c_void, message: Str) -> Status,

    /// From inside a hook callback: record why this callback failed, and end the dispatch.
    ///
    /// Returns [`Status::Error`], the code the callback then returns. The message is copied
    /// immediately and becomes the error whoever fired the hook receives.
    pub stop_with_error: unsafe extern "C" fn(host_data: *mut c_void, message: Str) -> Status,

    /// Publish a function under this plugin's name. Writes the new registration id to `out_id`.
    pub export: unsafe extern "C" fn(
        host_data: *mut c_void,
        name: Str,
        function: ApiFunction,
        out_id: *mut u64,
        error_out: *mut Str,
    ) -> Status,

    /// Withdraw one of this plugin's own functions.
    pub unexport: unsafe extern "C" fn(host_data: *mut c_void, id: u64) -> Status,

    /// Call `plugin::function`. On success `out` receives an owned handle; on [`Status::Error`] it
    /// receives the callee's free-form error value, or stays null when the framework itself
    /// refused the call — and then `error_out` says why.
    pub plugin_call: unsafe extern "C" fn(
        host_data: *mut c_void,
        target: Str,
        args: *const ValueHandle,
        out: *mut *mut ValueHandle,
        error_out: *mut Str,
    ) -> Status,

    /// Call one of the application's own functions, by flat name. Same `out` contract as
    /// [`plugin_call`](Self::plugin_call).
    pub host_call: unsafe extern "C" fn(
        host_data: *mut c_void,
        name: Str,
        args: *const ValueHandle,
        out: *mut *mut ValueHandle,
        error_out: *mut Str,
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

    /// Passed back as the first argument of every [`HostApi`] call. Opaque to the plugin.
    ///
    /// One per loaded plugin, not one per host: it is how the host knows *which* plugin is calling
    /// as well as which host is being called, so nothing a plugin registers has to name its owner.
    pub host_data: *mut c_void,

    /// The name the host knows this plugin by. It is the plugin's identity: everything it
    /// registers is tagged with it, and other plugins address its functions through it. Valid for
    /// the life of the process.
    pub plugin_name: Str,
}
