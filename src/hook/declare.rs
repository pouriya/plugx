use std::sync::atomic::{AtomicU64, Ordering};

/// FNV-1a over the hook name. Const-evaluable, so [`Hook::new`] costs nothing at runtime.
const fn hash_name(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

/// The bit this hook occupies in the registry's 64-bit presence filter.
pub(crate) const fn filter_bit(name: &str) -> u64 {
    1u64 << (hash_name(name) % 64)
}

/// A hook declared once, with its filter bit computed at compile time and its position in the
/// registry cached after the first dispatch.
///
/// Declaring a hook is optional — `run("name", …)` works everywhere a `&Hook` does. It is worth
/// doing for hooks fired in a hot loop, because it turns "is anything registered for this?" into a
/// single relaxed atomic load, with no string hashing and no table lookup.
///
/// Build one with [`hook!`](macro@crate::hook), which also gives it a name you can grep for.
#[derive(Debug)]
pub struct Hook {
    name: &'static str,
    bit: u64,
    /// Packed `(generation + 1) << 32 | index`. Zero means "never resolved". The generation guards
    /// the index: any change to the table's shape bumps it, invalidating every cache at once.
    cache: AtomicU64,
}

impl Hook {
    /// Declare a hook. `const`, so this can initialise a `static`.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            bit: filter_bit(name),
            cache: AtomicU64::new(0),
        }
    }

    /// The hook's name, as it appears in the registry and in logs.
    pub const fn name(&self) -> &str {
        self.name
    }

    /// This hook's bit in the presence filter.
    pub(crate) const fn bit(&self) -> u64 {
        self.bit
    }

    /// The cached table index for `generation`, if this hook has been resolved since that
    /// generation began.
    pub(crate) fn cached(&self, generation: u32) -> Option<usize> {
        let packed = self.cache.load(Ordering::Relaxed);
        if packed == 0 {
            return None;
        }
        let cached_generation = (packed >> 32) as u32;
        if cached_generation != generation.wrapping_add(1) {
            return None;
        }
        Some((packed & 0xFFFF_FFFF) as usize)
    }

    /// Remember that this hook sits at `index` for as long as `generation` lasts.
    pub(crate) fn cache(&self, generation: u32, index: usize) {
        if index > u32::MAX as usize {
            return;
        }
        let packed = (u64::from(generation.wrapping_add(1)) << 32) | index as u64;
        self.cache.store(packed, Ordering::Relaxed);
    }
}

/// A hook named at a call site, either by a declared [`Hook`] or by a bare string.
///
/// Every entry point takes `impl Into<HookRef<'_>>`, so `run("request.headers", &mut data)` and
/// `run(&REQUEST_HEADERS, &mut data)` are both spelled the way you would expect.
#[derive(Debug, Clone, Copy)]
pub enum HookRef<'a> {
    /// A hook declared with [`hook!`](macro@crate::hook), carrying a precomputed filter bit and an index
    /// cache.
    Declared(&'a Hook),
    /// A hook named inline. The filter bit is computed on the spot and nothing is cached.
    Named(&'a str),
}

impl<'a> HookRef<'a> {
    /// The hook's name.
    pub const fn name(&self) -> &'a str {
        match self {
            Self::Declared(hook) => hook.name(),
            Self::Named(name) => name,
        }
    }

    /// This hook's bit in the presence filter.
    pub(crate) const fn bit(&self) -> u64 {
        match self {
            Self::Declared(hook) => hook.bit(),
            Self::Named(name) => filter_bit(name),
        }
    }

    /// The declared hook behind this reference, if there is one.
    pub(crate) const fn declared(&self) -> Option<&'a Hook> {
        match self {
            Self::Declared(hook) => Some(hook),
            Self::Named(_) => None,
        }
    }
}

impl<'a> From<&'a Hook> for HookRef<'a> {
    fn from(hook: &'a Hook) -> Self {
        Self::Declared(hook)
    }
}

impl<'a> From<&'a str> for HookRef<'a> {
    fn from(name: &'a str) -> Self {
        Self::Named(name)
    }
}

impl<'a> From<&'a String> for HookRef<'a> {
    fn from(name: &'a String) -> Self {
        Self::Named(name)
    }
}

/// Declare one or more hooks as statics, with their filter bits computed at compile time.
///
/// ```rust
/// plugx::hook!(
///     /// Fired once per request, before routing.
///     pub REQUEST_HEADERS = "request.headers";
///     pub(crate) RESPONSE_BODY = "response.body";
/// );
/// assert_eq!(REQUEST_HEADERS.name(), "request.headers");
/// ```
#[macro_export]
macro_rules! hook {
    ($(
        $(#[$meta:meta])*
        $visibility:vis $ident:ident = $name:expr
    );+ $(;)?) => {
        $(
            $(#[$meta])*
            $visibility static $ident: $crate::Hook = $crate::Hook::new($name);
        )+
    };
}
