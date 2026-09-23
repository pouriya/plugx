use crate::abi::AbiVersion;
use crate::abi::cdylib::context::Context;
use crate::abi::cdylib::primitive::{Status, Str};
use crate::abi::cdylib::value::ValueHandle;

/// The symbol a plugin exports to report the ABI it was built against, as a NUL-terminated name.
///
/// The host resolves and calls this **first**, before anything else in the library. It touches no
/// host state and allocates nothing, so it is safe to call on a library that turns out to be
/// incompatible and is then abandoned.
pub const VERSION_SYMBOL: &[u8] = b"plugx_abi_version\0";

/// The symbol a plugin exports to describe itself. Signature: [`InfoFn`].
pub const INFO_SYMBOL: &[u8] = b"plugx_info\0";

/// The symbol a plugin exports to be brought up. Signature: [`StartFn`].
pub const START_SYMBOL: &[u8] = b"plugx_start\0";

/// The symbol a plugin exports to take a new configuration. Signature: [`ReloadFn`].
pub const RELOAD_SYMBOL: &[u8] = b"plugx_reload\0";

/// The symbol a plugin exports to be torn down. Signature: [`StopFn`].
pub const STOP_SYMBOL: &[u8] = b"plugx_stop\0";

/// The signature of [`VERSION_SYMBOL`].
pub type VersionFn = unsafe extern "C" fn() -> AbiVersion;

// Every signature below but `VersionFn` takes an `error_out` last, and writes a message into it
// before returning anything but `Status::Ok`. The host copies it immediately, so a `static` or
// thread-local buffer in the plugin is enough; `error_out` may be null, and a plugin with nothing
// to say leaves it alone. Nobody ever calls back to ask: that is what `plugx_last_error` used to
// be, and it cost two crossings for one message.

/// The signature of [`INFO_SYMBOL`].
///
/// Reports the plugin's version, description, configuration spec and dependencies as a value tree.
/// The tree is allocated with the host's own [`ValueApi`](crate::abi::cdylib::ValueApi), so ownership of
/// the handle written to `out` passes to the host, which releases it.
pub type InfoFn = unsafe extern "C" fn(
    context: *const Context,
    out: *mut *mut ValueHandle,
    error_out: *mut Str,
) -> Status;

/// The signature of [`START_SYMBOL`].
///
/// Brings the plugin up with `config` and lets it register its hook callbacks and export its
/// functions — all through the [`Context`] passed here, which is the only thing that leads back to
/// the host.
pub type StartFn = unsafe extern "C" fn(
    context: *const Context,
    config: *const ValueHandle,
    error_out: *mut Str,
) -> Status;

/// The signature of [`RELOAD_SYMBOL`].
///
/// A **configuration** reload, not a code reload: the same library stays mapped and the plugin's
/// registrations stay in place unless it changes them itself.
pub type ReloadFn = unsafe extern "C" fn(
    context: *const Context,
    old_config: *const ValueHandle,
    new_config: *const ValueHandle,
    error_out: *mut Str,
) -> Status;

/// The signature of [`STOP_SYMBOL`].
///
/// The host calls this only after it has drained this plugin's callbacks and exported functions
/// and waited for every in-flight dispatch and call to finish, so nothing can still be running
/// against the state being torn down here.
pub type StopFn = unsafe extern "C" fn(context: *const Context, error_out: *mut Str) -> Status;
