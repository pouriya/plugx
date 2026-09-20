//! The handle everything else is reached through.
//!
//! A `Context` is 40 bytes: a name and a reach. The name is the identity — what the owner is
//! called, what logs print, and what everything it registers is tagged with. The reach is where
//! its tables are: pointed straight at them when the code is linked into the host, or at the
//! host's function-pointer table when the code is running inside a loaded shared library.
//!
//! Nobody stores one. It is `Copy`, `'static` and cheap enough to rebuild on the spot — the host
//! builds one per lifecycle call, and a dispatch builds one per callback out of the entry's owner
//! and the runner's own reach. A plugin that needs one on a background thread just keeps its copy.
//!
//! Code linked into the application can skip all this and use [`run`](crate::run), which reads the
//! program's tables from the one slot a host fills. That slot is empty inside a loaded library, so
//! a plugin registers, exports and calls through the context it was handed — the only thing in
//! there that leads anywhere.

use crate::error::Result;
use crate::hook::callback::{Flow, Observe, RegistrationId, Transform};
use crate::hook::declare::HookRef;
use crate::remote::Remote;
use crate::tables::{ApiFn, CallbackKind, Tables};
use crate::value::Value;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::sync::Arc;

/// Where a context's tables are.
///
/// This is the whole of what a context knows how to reach, and the two arms are the same thing
/// twice: `Local` is a pointer to the tables, and `Remote` is that same pointer — as `host_data` —
/// in the form that fits through `extern "C"`, alongside the vtable that takes it back. Nothing
/// else carries it, which is why a shared library has no table of its own to register into.
#[derive(Clone, Copy)]
pub(crate) enum Reach {
    /// The tables themselves, for code linked into the host that owns them.
    Local(&'static Tables),
    /// The host's vtable, for code running inside a library the host loaded.
    Remote(Remote),
}

/// What a plugin is handed on every lifecycle call, and every callback on every dispatch.
///
/// Through it an owner can register and remove hook callbacks, export and withdraw its own
/// functions, call another plugin's functions, call the application's functions, and fire hooks.
/// The same nine methods do the same nine things whether the code is statically linked into the
/// host or loaded from a shared library.
#[derive(Clone, Copy)]
pub struct Context {
    name: &'static str,
    reach: Reach,
}

impl Debug for Context {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        let reach = match self.reach {
            Reach::Local(_) => "local",
            Reach::Remote(_) => "remote",
        };
        formatter
            .debug_struct("Context")
            .field("name", &self.name)
            .field("reach", &reach)
            .finish()
    }
}

impl Context {
    /// Build a context for code linked into the host that owns `tables`.
    pub(crate) const fn local(name: &'static str, tables: &'static Tables) -> Self {
        Self {
            name,
            reach: Reach::Local(tables),
        }
    }

    /// Build a context for code running inside a library `host` loaded.
    pub(crate) const fn remote(name: &'static str, remote: Remote) -> Self {
        Self {
            name,
            reach: Reach::Remote(remote),
        }
    }

    /// Build a context for `name` over an already-known reach — what a dispatch hands each
    /// callback, and what a call hands the callee.
    pub(crate) const fn new(name: &'static str, reach: Reach) -> Self {
        Self { name, reach }
    }

    /// The name the host knows this owner by. Identity, not decoration: registrations, exports and
    /// `plugin::function` addressing all key off it.
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// Fire `hook`, running every callback registered for it in priority order.
    ///
    /// Accepts either a declared hook or a bare name. Firing a hook nobody has registered for
    /// costs one relaxed atomic load, a mask and a branch — sprinkle them freely.
    ///
    /// Each callback is handed **its own** context, so it can fire further hooks, call other
    /// plugins, and register or withdraw whatever it likes while the dispatch is still running.
    /// No lock is held while any of that happens.
    pub fn run<'a>(&self, hook: impl Into<HookRef<'a>>, data: &mut Value) -> Result<Flow> {
        let hook = hook.into();
        match self.reach {
            Reach::Local(tables) => tables.dispatch(self.reach, hook, data),
            Reach::Remote(remote) => remote.run(hook.name(), data),
        }
    }

    /// Register a callback that may rewrite a hook's payload.
    ///
    /// Lower `priority` runs earlier; ties run in registration order. The registration is tagged
    /// with this context's name, so the host removes it when the owner stops — a plugin does not
    /// have to unregister in its own `stop`.
    pub fn on_transform<'a>(
        &self,
        hook: impl Into<HookRef<'a>>,
        priority: i32,
        callback: impl Transform + 'static,
    ) -> Result<RegistrationId> {
        let hook = hook.into();
        let callback: Arc<dyn Transform> = Arc::new(callback);
        match self.reach {
            Reach::Local(tables) => Ok(tables.register(
                self.name,
                hook.name(),
                priority,
                CallbackKind::Transform(callback),
            )),
            Reach::Remote(remote) => {
                remote.register_transform(*self, hook.name(), priority, callback)
            }
        }
    }

    /// Register a callback that may only read a hook's payload.
    pub fn on_observe<'a>(
        &self,
        hook: impl Into<HookRef<'a>>,
        priority: i32,
        callback: impl Observe + 'static,
    ) -> Result<RegistrationId> {
        let hook = hook.into();
        let callback: Arc<dyn Observe> = Arc::new(callback);
        match self.reach {
            Reach::Local(tables) => Ok(tables.register(
                self.name,
                hook.name(),
                priority,
                CallbackKind::Observe(callback),
            )),
            Reach::Remote(remote) => {
                remote.register_observe(*self, hook.name(), priority, callback)
            }
        }
    }

    /// Remove one of this owner's hook registrations early. Returns whether it was there.
    pub fn unregister(&self, registration: RegistrationId) -> bool {
        match self.reach {
            Reach::Local(tables) => tables.unregister(self.name, registration),
            Reach::Remote(remote) => remote.unregister(self.name, registration),
        }
    }

    /// Publish a function under this owner's name, callable by anyone as `name::function`.
    ///
    /// May be called at any time, including from inside a callback. Exporting a name this owner
    /// already has live is [`Error::Duplicate`](crate::Error::Duplicate); withdraw the old one
    /// first with [`unexport`](Self::unexport).
    pub fn export(&self, name: &str, function: impl ApiFn + 'static) -> Result<RegistrationId> {
        let function: Arc<dyn ApiFn> = Arc::new(function);
        match self.reach {
            Reach::Local(tables) => tables.export(self.name, name, function),
            Reach::Remote(remote) => remote.export(*self, name, function),
        }
    }

    /// Withdraw one of this owner's functions. Returns whether it was there.
    pub fn unexport(&self, registration: RegistrationId) -> bool {
        match self.reach {
            Reach::Local(tables) => tables.unexport(self.name, registration),
            Reach::Remote(remote) => remote.unexport(self.name, registration),
        }
    }

    /// Call another plugin's function, named `plugin::function`.
    ///
    /// One value in, one value out; the argument is always copied, even between two plugins
    /// statically linked into the host, so there is one code path for every kind of plugin.
    ///
    /// The callee runs with its own context, not this one — the caller's identity is not exposed.
    /// A callee is free to fire hooks and call further plugins, including back into this one;
    /// plugx does not detect that cycle, and a plugin author who builds one owns it.
    pub fn plugin_call(&self, target: &str, args: Value) -> Result<Value> {
        match self.reach {
            Reach::Local(tables) => tables.plugin_call(self.reach, target, args),
            Reach::Remote(remote) => remote.plugin_call(target, args),
        }
    }

    /// Call one of the application's own functions. Flat names, no plugin prefix.
    pub fn host_call(&self, name: &str, args: Value) -> Result<Value> {
        match self.reach {
            Reach::Local(tables) => tables.host_call(self.reach, name, args),
            Reach::Remote(remote) => remote.host_call(name, args),
        }
    }
}
