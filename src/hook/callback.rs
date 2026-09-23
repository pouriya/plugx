use crate::context::Context;
use crate::error::Result;
use crate::value::Value;

/// What a callback tells the dispatcher when it returns.
///
/// Two independent things at once: **where dispatch goes next**, which is the variant, and
/// **whether this callback failed**, which is the `Result` it carries. They are independent on
/// purpose — a callback that broke can still let the rest of the chain run, and one that succeeded
/// can still end the dispatch.
///
/// All four spellings mean something different:
///
/// | Returned | Dispatch | What `run` gets back |
/// |----------|----------|----------------------|
/// | `Continue(Ok(()))` | runs the next callback | — |
/// | `Continue(Err(e))` | runs the next callback | — (`e` is logged against the plugin) |
/// | `Stop(Ok(()))` | ends here | `Ok(())` |
/// | `Stop(Err(e))` | ends here | `Err(e)` |
///
/// Only a `Stop` reaches the caller. A `Continue(Err(…))` is the way to say "I broke, but do not
/// let that ruin it for everybody else" — it is logged at `warn` with the plugin's name, and the
/// dispatch carries on.
#[derive(Debug)]
pub enum Flow {
    /// Run the next callback for this hook.
    Continue(Result<()>),
    /// Stop here. Callbacks after this one do not run, and this is what `run` returns.
    Stop(Result<()>),
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
    /// Run against the payload. See [`Flow`] for what the four answers mean.
    fn call(&self, context: &Context, data: &mut Value) -> Flow;
}

/// A callback that may read the payload but not change it.
pub trait Observe: Send + Sync {
    /// Run against the payload. See [`Flow`] for what the four answers mean.
    fn call(&self, context: &Context, data: &Value) -> Flow;
}

impl<F> Transform for F
where
    F: Fn(&Context, &mut Value) -> Flow + Send + Sync,
{
    fn call(&self, context: &Context, data: &mut Value) -> Flow {
        self(context, data)
    }
}

impl<F> Observe for F
where
    F: Fn(&Context, &Value) -> Flow + Send + Sync,
{
    fn call(&self, context: &Context, data: &Value) -> Flow {
        self(context, data)
    }
}
