//! # SDK
//!
//! Write a plugin in Rust, build it as a `cdylib`, and let a plugx host load it.
//!
//! ```rust,ignore
//! use plugx::sdk::prelude::*;
//!
//! #[derive(Default)]
//! struct Redact;
//!
//! impl Plugin for Redact {
//!     fn info(&self, _: &Context) -> Result<Info> {
//!         Ok(Info::new(Version::new(1, 0, 0), "Redacts secrets from log records"))
//!     }
//!
//!     fn start(&self, context: &Context, _config: &Value) -> Result<()> {
//!         context.on_transform("log.record", 0, |_: &Context, data: &mut Value| {
//!             if let Some(map) = data.as_map_mut() {
//!                 map.insert("password", Value::Str("[redacted]".into()));
//!             }
//!             Flow::Continue(Ok(()))
//!         })?;
//!         context.export("redact", |_: &Context, args: Value| Ok(args))?;
//!         Ok(())
//!     }
//!
//!     fn stop(&self, _: &Context) -> Result<()> { Ok(()) }
//! }
//!
//! plugx::export_plugin!(Redact);
//! ```
//!
//! ```toml
//! [lib]
//! crate-type = ["cdylib"]
//! ```
//!
//! # What `export_plugin!` does for you
//!
//! It exports one symbol per lifecycle operation — `plugx_abi_version`, `plugx_info`,
//! `plugx_start`, `plugx_reload`, `plugx_stop`, `plugx_last_error` — and keeps your plugin value
//! in a `static` in your own library. There is no entry point, nothing is installed, and nothing
//! is stored on your behalf.
//!
//! Each symbol is handed the host's context and turns it into the [`Context`](crate::Context) your
//! method is called with: a name, and the host's function table. That is the whole mechanism, and
//! it is why there is no setup step to forget. Registering, exporting, calling another plugin and
//! firing a hook all go through that context. If you want one on a background thread, keep a copy
//! — it is `Copy` and `'static`.

/// Everything a plugin author needs, in one `use`.
pub mod prelude {
    pub use crate::context::Context;
    pub use crate::hook::{Flow, Observe, RegistrationId, Transform};
    pub use crate::plugin::{ConfigSpec, Dependency, Error, Info, Plugin, Result, Version};
    pub use crate::value::{Kind, Map, Value};
}

/// Building a plugin as a shared library the host loads with `dlopen`.
#[cfg(feature = "compile-cdylib")]
pub mod cdylib;
