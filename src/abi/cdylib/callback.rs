use crate::abi::cdylib::primitive::Status;
use crate::abi::cdylib::value::ValueHandle;
use std::ffi::c_void;

/// A plugin's hook callback, as the host stores it.
///
/// `data` is a borrowed handle owned by the dispatching side. A transform may mutate the tree
/// through it; an observe callback must not. The handle is invalid the moment this returns.
///
/// Two arguments and no more, deliberately: this is the one thing in the ABI with no `size` field,
/// so it is the one thing that cannot grow. Everything a callback needs to know about itself —
/// which hook, whose callback, what else it can reach — is already inside the
/// [`Context`](crate::Context) the plugin side built at entry and kept, so none of it has to cross
/// here on every single dispatch.
pub type CallbackFn =
    unsafe extern "C" fn(user_data: *mut c_void, data: *mut ValueHandle) -> Status;

/// A plugin's exported function, as the host stores it.
///
/// `args` is borrowed for the call. On success the callee writes an owned handle to `out`; on
/// [`Status::Error`] it may write its free-form error value there instead, and the host turns that
/// into [`Error::Failed`](crate::Error::Failed).
pub type ApiCallFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    args: *const ValueHandle,
    out: *mut *mut ValueHandle,
) -> Status;

/// Releases a callback's or a function's `user_data`.
///
/// The host calls this exactly once, after the record has been removed from its table **and**
/// every in-flight dispatch or call that could still reach it has finished — and while the
/// plugin's library is still mapped.
pub type DropFn = unsafe extern "C" fn(user_data: *mut c_void);

/// One registered callback: what to call, what to pass it, and how to free it.
///
/// This, not a Rust trait object, is what a host stores on behalf of a loaded plugin. A
/// `Box<dyn Transform>` built inside a cdylib carries a vtable pointing into that library and a
/// `Drop` impl that lives there too; a plain record of function pointers does not care where the
/// code came from.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Callback {
    /// Invoked on dispatch.
    pub call: CallbackFn,
    /// Passed back to `call` and to `drop` untouched. Opaque to the host.
    pub user_data: *mut c_void,
    /// Frees `user_data`. `None` when there is nothing to free.
    pub drop: Option<DropFn>,
}

// SAFETY: `Callback` is a pair of function pointers and an opaque `user_data` the host never
// dereferences. Sending it between threads is sound because the host only ever moves it into and
// out of the table; whether the *plugin's* `user_data` tolerates concurrent calls is the plugin's
// own contract, documented on `CallbackFn`.
unsafe impl Send for Callback {}
// SAFETY: see the `Send` impl above. The host shares `&Callback` across dispatching threads and
// only reads the two pointers out of it.
unsafe impl Sync for Callback {}

/// One exported function: what to call, what to pass it, and how to free it.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ApiFunction {
    /// Invoked on a `plugin::function` call.
    pub call: ApiCallFn,
    /// Passed back to `call` and to `drop` untouched. Opaque to the host.
    pub user_data: *mut c_void,
    /// Frees `user_data`. `None` when there is nothing to free.
    pub drop: Option<DropFn>,
}

// SAFETY: as `Callback` above — function pointers plus an opaque `user_data` the host never
// dereferences.
unsafe impl Send for ApiFunction {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for ApiFunction {}
