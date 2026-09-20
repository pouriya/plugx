//! # Values
//!
//! The dynamic value that flows through every plugx hook.
//!
//! [`Value`] is deliberately small — six variants, no borrowed lifetimes, no nesting tricks — so
//! that the same shape survives a trip through a C ABI, a WebAssembly module, a Starlark script, a
//! JavaScript engine or an HTTP webhook without losing meaning. [`Kind`] is `#[repr(u8)]` and is
//! the discriminant the ABI puts on the wire; everything else here is ordinary safe Rust.
//!
//! ```rust
//! use plugx::value::{Map, Value};
//!
//! let mut request = Value::Map(Map::new());
//! if let Some(map) = request.as_map_mut() {
//!     map.insert("path", Value::Str("/health".to_string()));
//!     map.insert("retries", Value::Int(3));
//! }
//!
//! if let Some(path) = request.get_path("path") {
//!     assert_eq!(path.as_str(), Some("/health"));
//! }
//! ```

/// Errors produced when reading a [`Value`].
pub mod error;
/// The [`Value`] tree and its [`Map`].
pub mod tree;

pub use error::{Error, ErrorKind};
pub use tree::{Kind, Map, Value};
