use crate::abi::cdylib::primitive::{Status, Str};
use std::marker::PhantomData;
use std::marker::PhantomPinned;

/// An opaque reference to a value tree owned by the host.
///
/// A plugin never sees the layout of a value and never allocates one itself — it asks the host,
/// through [`ValueApi`], to read and build them. That is what keeps the two sides' allocators
/// apart and lets the same vtable serve a WebAssembly module or a Starlark script, neither of
/// which can hold a host pointer.
///
/// Handles come in two flavours and confusing them is a double free:
///
/// - **Borrowed** — returned by [`ValueApi::list_get`] and [`ValueApi::map_get`], and passed into a
///   callback. Valid until the call that produced them returns. Never released.
/// - **Owned** — returned by the `new_*` constructors. Released with [`ValueApi::release`], or
///   handed to [`ValueApi::map_set`] / [`ValueApi::list_push`], which take ownership.
#[repr(C)]
pub struct ValueHandle {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// The host's value accessors, handed to a plugin inside [`crate::Context`].
///
/// Grows by appending; `size` says how much of it the host actually wrote.
#[repr(C)]
pub struct ValueApi {
    /// `size_of::<ValueApi>()` as the host built it. Check before reading an appended field.
    pub size: usize,

    /// The value's kind, as a `plugx::value::Kind` tag.
    pub kind: unsafe extern "C" fn(value: *const ValueHandle) -> u8,

    /// Read a boolean. [`Status::Error`] if the value is another kind.
    pub get_bool: unsafe extern "C" fn(value: *const ValueHandle, out: *mut bool) -> Status,
    /// Read an integer. [`Status::Error`] if the value is another kind.
    pub get_int: unsafe extern "C" fn(value: *const ValueHandle, out: *mut i64) -> Status,
    /// Read a float. [`Status::Error`] if the value is another kind.
    pub get_float: unsafe extern "C" fn(value: *const ValueHandle, out: *mut f64) -> Status,
    /// Borrow a string's bytes, valid until the current call returns.
    pub get_str: unsafe extern "C" fn(value: *const ValueHandle, out: *mut Str) -> Status,

    /// Number of items in a list.
    pub list_len: unsafe extern "C" fn(value: *const ValueHandle, out: *mut usize) -> Status,
    /// Borrow the item at `index`, or null if out of range. Never released by the caller.
    pub list_get: unsafe extern "C" fn(value: *mut ValueHandle, index: usize) -> *mut ValueHandle,
    /// Append `item` to a list, taking ownership of it.
    pub list_push: unsafe extern "C" fn(value: *mut ValueHandle, item: *mut ValueHandle) -> Status,

    /// Number of entries in a map.
    pub map_len: unsafe extern "C" fn(value: *const ValueHandle, out: *mut usize) -> Status,
    /// Borrow the key at `index` in insertion order, valid until the current call returns.
    pub map_key_at:
        unsafe extern "C" fn(value: *const ValueHandle, index: usize, out: *mut Str) -> Status,
    /// Borrow the value stored under `key`, or null if absent. Never released by the caller.
    pub map_get: unsafe extern "C" fn(value: *mut ValueHandle, key: Str) -> *mut ValueHandle,
    /// Store `item` under `key`, taking ownership of it and replacing anything already there.
    pub map_set:
        unsafe extern "C" fn(value: *mut ValueHandle, key: Str, item: *mut ValueHandle) -> Status,
    /// Remove `key`. [`Status::Error`] if it was not present.
    pub map_remove: unsafe extern "C" fn(value: *mut ValueHandle, key: Str) -> Status,

    /// Replace the value in place with a boolean, changing its kind.
    pub set_bool: unsafe extern "C" fn(value: *mut ValueHandle, item: bool) -> Status,
    /// Replace the value in place with an integer, changing its kind.
    pub set_int: unsafe extern "C" fn(value: *mut ValueHandle, item: i64) -> Status,
    /// Replace the value in place with a float, changing its kind.
    pub set_float: unsafe extern "C" fn(value: *mut ValueHandle, item: f64) -> Status,
    /// Replace the value in place with a copy of `item`, changing its kind.
    pub set_str: unsafe extern "C" fn(value: *mut ValueHandle, item: Str) -> Status,
    /// Replace the value in place with an empty list, changing its kind.
    pub set_list: unsafe extern "C" fn(value: *mut ValueHandle) -> Status,
    /// Replace the value in place with an empty map, changing its kind.
    pub set_map: unsafe extern "C" fn(value: *mut ValueHandle) -> Status,

    /// Allocate a new boolean. Owned by the caller.
    pub new_bool: unsafe extern "C" fn(item: bool) -> *mut ValueHandle,
    /// Allocate a new integer. Owned by the caller.
    pub new_int: unsafe extern "C" fn(item: i64) -> *mut ValueHandle,
    /// Allocate a new float. Owned by the caller.
    pub new_float: unsafe extern "C" fn(item: f64) -> *mut ValueHandle,
    /// Allocate a new string, copying `item`. Owned by the caller.
    pub new_str: unsafe extern "C" fn(item: Str) -> *mut ValueHandle,
    /// Allocate a new empty list. Owned by the caller.
    pub new_list: unsafe extern "C" fn() -> *mut ValueHandle,
    /// Allocate a new empty map. Owned by the caller.
    pub new_map: unsafe extern "C" fn() -> *mut ValueHandle,

    /// Free an owned handle. Never called on a borrowed one.
    pub release: unsafe extern "C" fn(value: *mut ValueHandle),
}

// SAFETY: `ValueApi` is a table of function pointers and a length. It carries no interior
// mutability and no owned memory; the host builds one and never mutates it afterwards, so sharing
// a `&ValueApi` across threads is sound. The safety of *calling* the pointers is each function's
// own documented contract.
unsafe impl Send for ValueApi {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for ValueApi {}
