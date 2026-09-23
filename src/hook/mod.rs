//! # Hooks
//!
//! Named extension points and the callbacks registered against them.
//!
//! Declare a hook where your library could be extended, fire it through a
//! [`Context`](crate::Context), and let
//! somebody else — the application, or a plugin the application loaded — decide what happens.
//!
//! ```rust
//! use plugx::{Context, Value};
//!
//! plugx::hook!(pub REQUEST_HEADERS = "request.headers");
//!
//! /// Anything that can be extended takes a context and fires through it.
//! fn handle(context: &Context) -> plugx::Result<()> {
//!     let mut headers = Value::map();
//!     context.run(&REQUEST_HEADERS, &mut headers)
//! }
//! ```
//!
//! Firing a hook nobody has registered for costs one relaxed atomic load, a mask and a branch — no
//! lock, no allocation, no `Arc` traffic. Sprinkle hooks freely.
//!
//! Declaring is optional: `context.run("request.headers", &mut data)` works everywhere a declared
//! hook does. It is worth doing for a hook fired in a hot loop, because the filter bit is then
//! computed at compile time and the table index is cached.
//!
//! # Two kinds of callback
//!
//! A [`Transform`] receives `&mut Value` and may rewrite the payload. An [`Observe`] receives
//! `&Value` and may only react. Both share one priority-ordered list per hook, so they interleave
//! however you schedule them, and both answer with a [`Flow`] — where the dispatch goes next, and
//! whether this callback failed, which are independent of each other.
//!
//! # Dispatch holds no lock
//!
//! [`Context::run`](crate::Context::run) copies the table snapshot pointer under a read lock held for a few
//! nanoseconds, then releases it and runs the callbacks. Any number of threads dispatch in
//! parallel, a callback may register another callback or fire a nested hook, and a slow callback
//! never blocks a registration.
//!
//! One dispatch sees one consistent set of callbacks for its whole duration. If a plugin is
//! stopped midway through, the callbacks already in flight still run to completion — the thread
//! calling `stop` waits for them, and no dispatching thread is slowed down.

/// The two callback traits, [`Flow`], and the id that identifies a registration.
pub mod callback;
/// Declaring a hook and naming one at a call site.
pub mod declare;

pub use callback::{Flow, Observe, RegistrationId, Transform};
pub use declare::{Hook, HookRef};
