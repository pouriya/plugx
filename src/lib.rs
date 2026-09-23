#![doc(test(no_crate_inject))]
#![deny(missing_docs)]

//! # plugx
//!
//! A plugin framework: named hooks, callbacks with priorities, plugin-to-plugin function calls,
//! and plugins loaded from shared libraries.
//!
//! # Two ways in
//!
//! Code linked into the application fires hooks with [`run`], which reads this program's registry
//! out of the one slot a [`Host`] fills. Code inside a loaded plugin is handed a [`Context`] on
//! every call and reaches the host through that. Through a context an owner can:
//!
//! - fire hooks — [`run`](Context::run)
//! - register and remove hook callbacks — [`on_transform`](Context::on_transform),
//!   [`on_observe`](Context::on_observe), [`unregister`](Context::unregister)
//! - publish and withdraw its own functions — [`export`](Context::export),
//!   [`unexport`](Context::unexport)
//! - call another plugin's functions — [`plugin_call`](Context::plugin_call)
//! - call the application's functions — [`host_call`](Context::host_call)
//!
//! A `Context` is 32 bytes, `Copy` and `'static`: a name and where its registry is. Nothing stores
//! one on a plugin's behalf — it is rebuilt per call — but a plugin that wants to fire a hook from
//! a background thread can keep its copy.
//!
//! [`run`] is deliberately the only free function. A plugin `.so` links its own copy of this
//! crate, so the slot inside it is empty and stays empty; a `run` from in there returns
//! [`Error::NoHost`] rather than vanishing into a private table, and everything else a plugin does
//! goes through the context it was given.
//!
//! # If you write a library
//!
//! Fire a hook wherever somebody might want to change what your library does. This is the default
//! feature set and pulls in nothing heavier than the registry. Take a `&Context` and fire through it
//! instead if your library might be compiled into a plugin, where there is no slot to read.
//!
//! ```toml
//! plugx = "0.1"
//! ```
//!
//! ```rust
//! use plugx::{Flow, Result, Value};
//!
//! plugx::hook!(pub REQUEST_HEADERS = "request.headers");
//!
//! fn handle(headers: &mut Value) -> Result<Flow> {
//!     plugx::run(&REQUEST_HEADERS, headers)
//! }
//! ```
//!
//! A `&str` works everywhere a declared hook does — `plugx::run("request.headers", &mut headers)`
//! — so nothing has to be declared up front. Declaring is worth it for a hook fired in a hot loop,
//! because the filter bit is then computed at compile time.
//!
//! Firing a hook nobody has registered for costs one relaxed atomic load, a mask and a branch.
//!
//! # If you write an application
//!
//! Take the host, whichever runtimes your plugins are built as, and whichever loaders fetch them.
//!
//! ```toml
//! plugx = { version = "0.1", features = ["runtime-cdylib", "load-file"] }
//! ```
//!
//! ```rust,ignore
//! let mut host = plugx::Host::new()?;
//! host.export("now", |_: &plugx::Context, _args| Ok(plugx::Value::Int(now())))?;
//! host.add_dir("plugins");
//! host.load_all()?;
//! host.start_all(&configs)?;
//! // ... run your program; anything that fires hooks uses plugx::run ...
//! host.stop_all()?;
//! ```
//!
//! # If you write a plugin
//!
//! Take the `compile-cdylib` feature, implement [`Plugin`], and build a `cdylib`.
//!
//! ```toml
//! plugx = { version = "0.1", features = ["compile-cdylib"] }
//!
//! [lib]
//! crate-type = ["cdylib"]
//! ```
//!
//! # Two kinds of callback
//!
//! A [`Transform`] receives `&mut Value` and may rewrite the payload. An [`Observe`] receives
//! `&Value` and may only react. They share one priority-ordered list per hook, and either can end
//! a dispatch early by returning [`Flow::Stop`].
//!
//! # What is guaranteed
//!
//! - **No lock is held while anything runs.** Any number of threads dispatch in parallel; a
//!   callback may register another, fire a nested hook, or call into a second plugin whose code
//!   fires back into the first; a slow callback blocks nobody.
//! - **A dispatch sees one consistent set of callbacks** for its whole duration. Stopping a plugin
//!   mid-dispatch lets the callbacks already in flight finish — only the thread calling `stop`
//!   waits.
//! - **A loaded library is never unloaded.** Stopping a plugin drains its callbacks and its
//!   exported functions, waits for both to go quiet, and drops them while its code is still
//!   mapped; the mapping itself stays for the life of the process. Leaking is always safe;
//!   `dlclose` is not.
//! - **Cross-plugin cycles are not detected.** A calls B, B fires a hook, the hook re-enters A:
//!   plugx lets it happen and the plugin author owns the consequences.

pub mod abi;
pub mod context;
pub mod error;
pub mod hook;
pub mod registry;
pub mod value;

#[cfg(feature = "plugin")]
pub mod plugin;

#[cfg(feature = "host")]
pub mod host;

#[cfg(feature = "compile-cdylib")]
pub mod sdk;

#[cfg(feature = "testing")]
pub mod testing;

pub use context::Context;
pub use error::{Error, Result};
pub use hook::{Flow, Hook, HookRef, Observe, RegistrationId, Transform};
pub use registry::{ApiFn, Registry, State, run};
pub use value::{Kind, Map, Value};

pub use abi::AbiVersion;

#[cfg(any(feature = "runtime-cdylib", feature = "compile-cdylib"))]
pub use abi::cdylib::{ABI_VERSION, Context as AbiContext};

#[cfg(feature = "plugin")]
pub use plugin::{ConfigSpec, Dependency, Info, Plugin, Version};

#[cfg(feature = "host")]
pub use host::Host;
