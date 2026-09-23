use crate::abi::cdylib::AbiVersion;

/// The C ABI this build of the crate speaks.
///
/// A plugin reports the version it was compiled against; a host refuses to load a plugin whose
/// [`AbiVersion::major`] differs from its own.
pub const ABI_VERSION: AbiVersion = AbiVersion {
    major: 2,
    minor: 0,
    patch: 0,
};

/// A borrowed string: a pointer and a length, with no NUL terminator.
///
/// This is how every piece of text crosses the boundary — a hook name, a map key, a log line, an
/// error message. Pointer plus length rather than a C string, because a Rust `str` is not
/// NUL-terminated and may legally contain an interior NUL: terminating one would mean allocating a
/// copy on every crossing, and trusting one coming back would mean an unbounded read.
///
/// **UTF-8 is expected but not enforced.** The bytes are whatever the other side wrote; every
/// conversion here checks them and reports `None` rather than assuming.
///
/// A `Str` is only valid for the duration of the call it was passed to. **The receiver copies it
/// immediately** — retaining the pointer past the call is a use-after-free, because the memory
/// belongs to the other side of the boundary.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Str {
    /// Start of the bytes. May be null only when `len` is zero.
    pub ptr: *const u8,
    /// Length in bytes, not counting any terminator — there is none.
    pub len: usize,
}

impl Str {
    /// The empty string.
    pub const EMPTY: Self = Self {
        ptr: std::ptr::null(),
        len: 0,
    };

    /// Borrow `source`, valid for as long as `source` is.
    pub const fn from_str(source: &str) -> Self {
        Self {
            ptr: source.as_ptr(),
            len: source.len(),
        }
    }

    /// Borrow the bytes as a `&'static str`, or `None` if they are not UTF-8.
    ///
    /// Copies nothing. Only sound for a `Str` the other side documents as living for the whole
    /// process — [`plugin_name`](crate::abi::cdylib::Context::plugin_name) is the one such `Str` in
    /// this ABI, because the host leaks the name it loaded a plugin under.
    ///
    /// # Safety
    ///
    /// `ptr` must point at `len` initialised, readable bytes that stay valid for the life of the
    /// process, not merely for the current call.
    pub unsafe fn as_static_str(self) -> Option<&'static str> {
        if self.len == 0 {
            return Some("");
        }
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees `ptr` covers `len` readable bytes that outlive the
        // process, which is exactly the lifetime claimed here.
        let bytes = unsafe { std::slice::from_raw_parts(self.ptr, self.len) };
        std::str::from_utf8(bytes).ok()
    }

    /// Copy the bytes out into an owned `String`, or `None` if they are not UTF-8.
    ///
    /// # Safety
    ///
    /// `ptr` must point at `len` initialised, readable bytes that stay valid for this call.
    pub unsafe fn to_string_lossless(self) -> Option<String> {
        if self.len == 0 {
            return Some(String::new());
        }
        if self.ptr.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees `ptr` covers `len` readable bytes for this call, and we
        // copy out of the borrow before returning.
        let bytes = unsafe { std::slice::from_raw_parts(self.ptr, self.len) };
        match std::str::from_utf8(bytes) {
            Ok(text) => Some(text.to_string()),
            Err(_) => None,
        }
    }
}

/// The result of a call across the boundary.
///
/// Negative values are failures; a receiver that sees an unknown negative treats it as
/// [`Status::Error`].
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Succeeded; keep going.
    Ok = 0,
    /// Succeeded, and no further callback should run for this hook.
    Stop = 1,
    /// Failed. The caller reads the detail with the host's error accessor.
    Error = -1,
    /// The plugin does not implement the requested operation.
    Unsupported = -2,
    /// A `size` field was smaller than the fields this side needs to read.
    Incompatible = -3,
}

impl Status {
    /// Recover a status from its wire value, mapping anything unknown to [`Status::Error`].
    pub const fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Ok,
            1 => Self::Stop,
            -2 => Self::Unsupported,
            -3 => Self::Incompatible,
            _ => Self::Error,
        }
    }
}
