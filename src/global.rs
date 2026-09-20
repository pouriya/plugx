//! The one global: where this program's tables are.
//!
//! A [`Host`](crate::Host) puts its tables here when it is built and takes them out when it is
//! dropped, and [`run`] is what reads them. That is the whole of it — one pointer, so that a
//! library with hooks in it can fire them without being handed anything:
//!
//! ```rust,ignore
//! plugx::run("request.headers", &mut headers)?;
//! ```
//!
//! # Why one, and why only here
//!
//! A plugin `.so` links its own copy of this crate, so it gets its own copy of this slot — and
//! that copy stays empty, because nothing inside a loaded library ever fills it. Code in a plugin
//! is handed a [`Context`](crate::Context) on every call and reaches the host through that. A
//! `run` from inside a plugin therefore does not silently vanish into a private table: it returns
//! [`Error::NoHost`], which is the difference between this design and the
//! one it replaces.
//!
//! It also means one live `Host` per process. A second one, while the first is alive, is refused.

use crate::error::{Error, Result};
use crate::hook::callback::Flow;
use crate::hook::declare::HookRef;
use crate::tables::Tables;
use crate::value::Value;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};

/// This program's tables, or null before a host exists.
static TABLES: AtomicPtr<Tables> = AtomicPtr::new(null_mut());

/// Claim the slot for `tables`. Fails if another host already holds it.
pub(crate) fn install(tables: &'static Tables) -> bool {
    let pointer = std::ptr::from_ref(tables).cast_mut();
    TABLES
        .compare_exchange(null_mut(), pointer, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Give the slot back. Only the host that claimed it calls this, from its own `Drop`.
pub(crate) fn uninstall(tables: &'static Tables) {
    let pointer = std::ptr::from_ref(tables).cast_mut();
    let _ = TABLES.compare_exchange(pointer, null_mut(), Ordering::AcqRel, Ordering::Acquire);
}

/// This program's tables, if a host is alive.
pub(crate) fn tables() -> Option<&'static Tables> {
    let pointer = TABLES.load(Ordering::Acquire);
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the only non-null value this slot ever holds is a `&'static Tables` a host leaked,
    // and leaked memory stays valid even after that host is dropped and the slot cleared.
    Some(unsafe { &*pointer.cast_const() })
}

/// Fire a hook into this program's tables.
///
/// Accepts either a declared hook or a bare name. Firing a hook nobody has registered for costs
/// one relaxed atomic load, a mask and a branch, on top of one more to find the tables.
///
/// This is for code linked into the application: a library of yours with extension points in it
/// fires them this way, without the application having to thread anything through. Inside a
/// loaded plugin there are no tables to find and this returns [`Error::NoHost`] — plugin code
/// fires hooks through the [`Context`](crate::Context) it was handed.
pub fn run<'a>(hook: impl Into<HookRef<'a>>, data: &mut Value) -> Result<Flow> {
    match tables() {
        Some(tables) => tables.dispatch(crate::context::Reach::Local(tables), hook.into(), data),
        None => Err(Error::NoHost),
    }
}
