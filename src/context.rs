//! The handle everything else is reached through.
//!
//! A `Context` is 32 bytes: a name and a [`HostAccess`]. The name is the identity — what the owner
//! is called, what logs print, and what everything it registers is tagged with. The access is where
//! its registry is: pointed straight at it when the code is linked into the host, or at a
//! [`HostOps`] that leads back to them when the code runs outside the host's binary.
//!
//! Nobody stores one. It is `Copy`, `'static` and cheap enough to rebuild on the spot — the host
//! builds one per lifecycle call, and a dispatch builds one per callback out of the entry's owner
//! and the runner's own access. A plugin that needs one on a background thread just keeps its copy.
//!
//! Code linked into the application can skip all this and use [`run`](crate::run), which reads the
//! program's registry from the one slot a host fills. That slot is empty outside the host's binary,
//! so a plugin registers, exports and calls through the context it was handed — the only thing in
//! there that leads anywhere.

use crate::error::Result;
use crate::hook::callback::{Observe, RegistrationId, Transform};
use crate::hook::declare::HookRef;
use crate::registry::{ApiFn, CallbackKind, Registry};
use crate::value::Value;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::sync::Arc;

/// Everything a [`Context`] can ask of a host that is *not* this program's own [`Registry`].
///
/// One implementation per plugin kind that runs outside the host binary. A cdylib's implementation
/// calls the host's `extern "C"` function pointers; a WebAssembly guest's would call its imports.
/// Nothing in this module knows which — that is the whole point of the trait being here.
///
/// The nine methods mirror [`Context`]'s own, because a context is exactly this or the registry.
pub(crate) trait HostOps: Send + Sync {
    /// Fire `hook` in the host, and take back whatever its callbacks did to the payload.
    fn run(&self, hook: &str, data: &mut Value) -> Result<()>;
    /// File a transform callback in the host's registry under `context`'s name.
    fn register_transform(
        &self,
        context: Context,
        hook: &str,
        priority: i32,
        callback: Arc<dyn Transform>,
    ) -> Result<RegistrationId>;
    /// File an observe callback in the host's registry under `context`'s name.
    fn register_observe(
        &self,
        context: Context,
        hook: &str,
        priority: i32,
        callback: Arc<dyn Observe>,
    ) -> Result<RegistrationId>;
    /// Withdraw one of `owner`'s registrations. Whether it was there.
    fn unregister(&self, owner: &str, registration: RegistrationId) -> bool;
    /// Publish a function under `context`'s name.
    fn export(
        &self,
        context: Context,
        name: &str,
        function: Arc<dyn ApiFn>,
    ) -> Result<RegistrationId>;
    /// Withdraw one of `owner`'s functions. Whether it was there.
    fn unexport(&self, owner: &str, registration: RegistrationId) -> bool;
    /// Call `plugin::function` through the host.
    fn plugin_call(&self, target: &str, args: Value) -> Result<Value>;
    /// Call one of the application's own functions through the host.
    fn host_call(&self, name: &str, args: Value) -> Result<Value>;
}

/// How a context gets at the host's registry.
///
/// The two arms are the same thing twice: `Direct` is a pointer to the registry, and `Foreign` is
/// that same pointer in whatever form the plugin's kind can carry it — for a cdylib, `host_data`
/// plus the function pointers that take it back. Nothing else carries it, which is why a loaded
/// plugin has no table of its own to register into.
#[derive(Clone, Copy)]
pub(crate) enum HostAccess {
    /// Straight at the registry, for code compiled into the host binary.
    Direct(&'static Registry),
    /// Back through the host, for code running outside its binary.
    Foreign(&'static dyn HostOps),
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
    access: HostAccess,
}

impl Debug for Context {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        let access = match self.access {
            HostAccess::Direct(_) => "direct",
            HostAccess::Foreign(_) => "foreign",
        };
        formatter
            .debug_struct("Context")
            .field("name", &self.name)
            .field("access", &access)
            .finish()
    }
}

impl Context {
    /// Build a context for code linked into the host that owns `registry`.
    pub(crate) const fn direct(name: &'static str, registry: &'static Registry) -> Self {
        Self {
            name,
            access: HostAccess::Direct(registry),
        }
    }

    /// Build a context for code running outside the host's binary, reaching it through `host`.
    pub(crate) const fn foreign(name: &'static str, host: &'static dyn HostOps) -> Self {
        Self {
            name,
            access: HostAccess::Foreign(host),
        }
    }

    /// Build a context for `name` over an already-known access — what a dispatch hands each
    /// callback, and what a call hands the callee.
    pub(crate) const fn new(name: &'static str, access: HostAccess) -> Self {
        Self { name, access }
    }

    /// The name the host knows this owner by. Identity, not decoration: registrations, exports and
    /// `plugin::function` addressing all key off it.
    pub const fn name(&self) -> &str {
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
    ///
    /// Returns what the callback that ended the dispatch returned — `Ok(())` if none did, or if
    /// the one that did succeeded. A callback that failed but asked to carry on is logged and does
    /// not show up here; see [`Flow`](crate::Flow).
    pub fn run<'a>(&self, hook: impl Into<HookRef<'a>>, data: &mut Value) -> Result<()> {
        let hook = hook.into();
        match self.access {
            HostAccess::Direct(registry) => registry.dispatch(self.access, hook, data),
            HostAccess::Foreign(host) => host.run(hook.name(), data),
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
        match self.access {
            HostAccess::Direct(registry) => Ok(registry.register(
                self.name,
                hook.name(),
                priority,
                CallbackKind::Transform(callback),
            )),
            HostAccess::Foreign(host) => {
                host.register_transform(*self, hook.name(), priority, callback)
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
        match self.access {
            HostAccess::Direct(registry) => Ok(registry.register(
                self.name,
                hook.name(),
                priority,
                CallbackKind::Observe(callback),
            )),
            HostAccess::Foreign(host) => {
                host.register_observe(*self, hook.name(), priority, callback)
            }
        }
    }

    /// Remove one of this owner's hook registrations early. Returns whether it was there.
    pub fn unregister(&self, registration: RegistrationId) -> bool {
        match self.access {
            HostAccess::Direct(registry) => registry.unregister(self.name, registration),
            HostAccess::Foreign(host) => host.unregister(self.name, registration),
        }
    }

    /// Publish a function under this owner's name, callable by anyone as `name::function`.
    ///
    /// May be called at any time, including from inside a callback. Exporting a name this owner
    /// already has live is [`Error::Duplicate`](crate::Error::Duplicate); withdraw the old one
    /// first with [`unexport`](Self::unexport).
    pub fn export(&self, name: &str, function: impl ApiFn + 'static) -> Result<RegistrationId> {
        let function: Arc<dyn ApiFn> = Arc::new(function);
        match self.access {
            HostAccess::Direct(registry) => registry.export(self.name, name, function),
            HostAccess::Foreign(host) => host.export(*self, name, function),
        }
    }

    /// Withdraw one of this owner's functions. Returns whether it was there.
    pub fn unexport(&self, registration: RegistrationId) -> bool {
        match self.access {
            HostAccess::Direct(registry) => registry.unexport(self.name, registration),
            HostAccess::Foreign(host) => host.unexport(self.name, registration),
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
        match self.access {
            HostAccess::Direct(registry) => registry.plugin_call(self.access, target, args),
            HostAccess::Foreign(host) => host.plugin_call(target, args),
        }
    }

    /// Call one of the application's own functions. Flat names, no plugin prefix.
    pub fn host_call(&self, name: &str, args: Value) -> Result<Value> {
        match self.access {
            HostAccess::Direct(registry) => registry.host_call(self.access, name, args),
            HostAccess::Foreign(host) => host.host_call(name, args),
        }
    }
}
