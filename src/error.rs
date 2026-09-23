//! One error for everything a [`Context`](crate::Context) can fail at.
//!
//! A dispatch, a registration, an export and a call into another plugin all come back as the same [`Error`], so a
//! plugin that does all four has one thing to match on. It is kept small enough to return by value
//! — the boxed payloads are the only fields that grow — because every one of those paths is on a
//! hot line.

use crate::value::Value;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// The result of anything reached through a [`Context`](crate::Context).
pub type Result<T> = std::result::Result<T, Error>;

/// Why a hook dispatch, a registration, an export or an API call failed.
///
/// Kept small enough to return by value: the boxed payloads are the only fields that grow.
#[derive(Debug)]
pub enum Error {
    /// A callback returned an error. Dispatch stops and the error reaches whoever called `run`.
    Callback {
        /// The hook that was being dispatched.
        hook: Box<str>,
        /// What the callback reported.
        source: Box<dyn StdError + Send + Sync>,
    },
    /// A call across the C boundary failed, in either direction: the host refusing something a
    /// loaded plugin asked for, or a plugin's callback returning a failure code.
    Ffi {
        /// The hook that was being dispatched, or the operation that was attempted.
        operation: Box<str>,
        /// What the other side reported, when it said anything.
        message: Box<str>,
    },
    /// A stop gave up waiting for in-flight dispatches.
    ///
    /// The owner's callbacks and functions are already out of the registry, so it can receive
    /// nothing new, but at least one call was still running when the deadline passed. Nothing was
    /// dropped and the plugin was not stopped; retry.
    StopTimeout {
        /// How many in-flight calls were still outstanding.
        outstanding: u32,
    },
    /// [`plugx::run`](crate::run) was called with no host alive to dispatch into.
    ///
    /// Either the application has not built its [`Host`](crate::Host) yet or has already dropped
    /// it, or the call came from inside a loaded plugin — where there is no registry to find and a
    /// hook must be fired through the [`Context`](crate::Context) the plugin was handed.
    NoHost,
    /// A [`plugin_call`](crate::Context::plugin_call) target was not spelled `plugin::function`.
    Malformed {
        /// What was asked for.
        target: Box<str>,
    },
    /// No plugin of that name is loaded.
    NoSuchPlugin {
        /// What was asked for.
        plugin: Box<str>,
    },
    /// The plugin is loaded but has not been started, so it has exported nothing yet.
    NotStarted {
        /// The plugin that was called.
        plugin: Box<str>,
    },
    /// The plugin is started but exports no function of that name.
    NoSuchFunction {
        /// The plugin that was called.
        plugin: Box<str>,
        /// The function that was asked for.
        function: Box<str>,
    },
    /// A live function of that name is already exported under this name.
    Duplicate {
        /// Whose table it is.
        plugin: Box<str>,
        /// The name that was already taken.
        function: Box<str>,
    },
    /// The function ran and reported a failure of its own, shaped however it liked.
    Failed {
        /// The plugin that was called.
        plugin: Box<str>,
        /// What it reported. Free-form; plugx mandates no shape.
        error: Box<Value>,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Callback { hook, source } => {
                write!(formatter, "callback for hook `{hook}` failed: {source}")
            }
            Self::Ffi { operation, message } => {
                if message.is_empty() {
                    write!(formatter, "`{operation}` failed across the plugin boundary")
                } else {
                    write!(
                        formatter,
                        "`{operation}` failed across the plugin boundary: {message}"
                    )
                }
            }
            Self::StopTimeout { outstanding } => write!(
                formatter,
                "timed out waiting for {outstanding} in-flight call(s) to finish"
            ),
            Self::NoHost => write!(
                formatter,
                "no plugx host is running in this program \u{2014} inside a plugin, fire hooks through \
                 the context instead"
            ),
            Self::Malformed { target } => {
                write!(formatter, "`{target}` is not a `plugin::function` target")
            }
            Self::NoSuchPlugin { plugin } => write!(formatter, "no plugin `{plugin}` is loaded"),
            Self::NotStarted { plugin } => {
                write!(formatter, "plugin `{plugin}` is loaded but not started")
            }
            Self::NoSuchFunction { plugin, function } => {
                write!(formatter, "plugin `{plugin}` exports no `{function}`")
            }
            Self::Duplicate { plugin, function } => write!(
                formatter,
                "`{plugin}` already exports a live function called `{function}`"
            ),
            Self::Failed { plugin, error } => {
                write!(formatter, "`{plugin}` failed with a {} value", error.kind())
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Callback { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}
