use crate::context::Context;
use crate::error::Result;
use crate::value::Value;

/// Whether dispatch should keep going after a callback returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Run the next callback for this hook.
    Continue,
    /// Stop here. Callbacks after this one do not run, and `run` reports [`Flow::Stop`].
    Stop,
}

/// A handle to one registration or one export, so it can be removed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RegistrationId(u64);

impl RegistrationId {
    /// Wrap a raw id.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The raw id, as it crosses the ABI.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A callback that may rewrite the payload.
///
/// Transforms and [`Observe`] callbacks share one priority-ordered list per hook, so a transform
/// can be scheduled to run before or after an observer as needed.
///
/// The [`Context`] is the callback's own — the same one its plugin's `start` was given. Through it
/// a callback can fire further hooks, call other plugins, and register or withdraw whatever it
/// likes, all while the dispatch that invoked it is still running.
pub trait Transform: Send + Sync {
    /// Run against the payload. Returning [`Flow::Stop`] ends the dispatch.
    fn call(&self, context: &Context, data: &mut Value) -> Result<Flow>;
}

/// A callback that may read the payload but not change it.
pub trait Observe: Send + Sync {
    /// Run against the payload. Returning [`Flow::Stop`] ends the dispatch.
    fn call(&self, context: &Context, data: &Value) -> Result<Flow>;
}

impl<F> Transform for F
where
    F: Fn(&Context, &mut Value) -> Result<Flow> + Send + Sync,
{
    fn call(&self, context: &Context, data: &mut Value) -> Result<Flow> {
        self(context, data)
    }
}

impl<F> Observe for F
where
    F: Fn(&Context, &Value) -> Result<Flow> + Send + Sync,
{
    fn call(&self, context: &Context, data: &Value) -> Result<Flow> {
        self(context, data)
    }
}
