//! The four tables a [`Context`] reaches, and the drain that empties them.
//!
//! There is no table in a global here. A [`Tables`] is owned — and leaked — by one
//! [`Host`](crate::Host), and the only way to reach it is through a [`Context`] the host built.
//! Two hosts in one process therefore have genuinely separate registries, and a plugin loaded from
//! a shared library cannot register into a private, permanently dead copy, because it never has
//! one: its context points at the host that loaded it.
//!
//! | Table | Key | Holds |
//! |-------|-----|-------|
//! | hooks | hook name | every callback registered for it, in priority order |
//! | apis | plugin name, then function name | that plugin's exported functions |
//! | hosts | function name | the application's own functions, flat, no prefix |
//! | states | plugin name | where the plugin is in its lifecycle |
//!
//! Each is an `RwLock<Arc<…>>`, cloned-and-swapped on write. A reader clones the `Arc` under a read
//! lock held for a few nanoseconds, releases it, and only then invokes anything — because a
//! callback is free to register another callback, export a function, or call into a second plugin,
//! and any of those would deadlock against a lock held across the call.

use crate::context::{Context, Reach};
use crate::error::{Error, Result};
use crate::hook::callback::{Flow, Observe, RegistrationId, Transform};
use crate::hook::declare::{HookRef, filter_bit};
use crate::value::Value;
use cfg_if::cfg_if;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

/// Where a plugin is in its lifecycle.
///
/// Recorded here, not just in the host, so that calling a plugin that is loaded but not yet
/// started reports [`Error::NotStarted`] rather than looking identical to a typo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Loaded and inspected, not yet started.
    Loaded,
    /// Started. Its callbacks are in the hook table and its functions are callable.
    Started,
    /// Stopped. Its library is still mapped, and always will be.
    Stopped,
}

impl State {
    /// The lowercase name of this state, as it appears in errors and logs.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Started => "started",
            Self::Stopped => "stopped",
        }
    }
}

/// One function reachable through [`Context::plugin_call`] or [`Context::host_call`].
///
/// One value in, one value out. The callee is handed **its own** context, not the caller's, so a
/// function it exports can fire hooks and call further plugins exactly as its `start` could.
pub trait ApiFn: Send + Sync {
    /// Run the function.
    fn call(&self, context: &Context, args: Value) -> Result<Value>;
}

impl<F> ApiFn for F
where
    F: Fn(&Context, Value) -> Result<Value> + Send + Sync,
{
    fn call(&self, context: &Context, args: Value) -> Result<Value> {
        self(context, args)
    }
}

/// What kind of callback an entry holds.
#[derive(Clone)]
pub(crate) enum CallbackKind {
    Transform(Arc<dyn Transform>),
    Observe(Arc<dyn Observe>),
}

impl CallbackKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Transform(_) => "transform",
            Self::Observe(_) => "observe",
        }
    }
}

/// One hook registration.
#[derive(Clone)]
struct Entry {
    id: u64,
    owner: &'static str,
    priority: i32,
    callback: CallbackKind,
}

/// Every callback registered for one hook, in the order they will run.
#[derive(Clone)]
struct HookEntries {
    name: String,
    entries: Vec<Entry>,
}

/// The hook table, as one immutable snapshot.
///
/// Never mutated in place. A writer clones it, edits the clone, and swaps the new `Arc` in — so a
/// dispatch that is already holding a snapshot keeps running against a consistent set of
/// callbacks, and the writer never has to wait for it.
#[derive(Clone)]
struct HookTable {
    /// Sorted by name, so a lookup is a binary search and an index is stable for a generation.
    hooks: Vec<HookEntries>,
    /// Bumped whenever `hooks` gains or loses an element, invalidating every cached index at once.
    generation: u32,
}

impl HookTable {
    const fn new() -> Self {
        Self {
            hooks: Vec::new(),
            generation: 0,
        }
    }

    /// Add `entry` under `hook`, keeping the entry list sorted by priority (ties keep insertion
    /// order, because ids only ever increase).
    fn insert(&mut self, hook: &str, entry: Entry) {
        let index = match self
            .hooks
            .binary_search_by(|slot| slot.name.as_str().cmp(hook))
        {
            Ok(index) => index,
            Err(index) => {
                self.hooks.insert(
                    index,
                    HookEntries {
                        name: hook.to_string(),
                        entries: Vec::new(),
                    },
                );
                self.generation = self.generation.wrapping_add(1);
                index
            }
        };
        let entries = &mut self.hooks[index].entries;
        let mut position = entries.len();
        for (offset, existing) in entries.iter().enumerate() {
            if existing.priority > entry.priority {
                position = offset;
                break;
            }
        }
        entries.insert(position, entry);
    }

    /// Remove every entry `predicate` accepts, dropping any hook left empty. Returns what was
    /// removed.
    fn take_matching(&mut self, predicate: impl Fn(&Entry) -> bool) -> Vec<Entry> {
        let mut removed = Vec::new();
        let mut emptied = false;
        for slot in &mut self.hooks {
            let mut kept = Vec::with_capacity(slot.entries.len());
            for entry in slot.entries.drain(..) {
                if predicate(&entry) {
                    removed.push(entry);
                } else {
                    kept.push(entry);
                }
            }
            slot.entries = kept;
            if slot.entries.is_empty() {
                emptied = true;
            }
        }
        if emptied {
            self.hooks.retain(|slot| !slot.entries.is_empty());
            self.generation = self.generation.wrapping_add(1);
        }
        removed
    }

    /// Where `hook` lives, using the declared hook's cached index when it is still valid.
    fn position_of(&self, hook: HookRef<'_>) -> Option<usize> {
        if let Some(declared) = hook.declared()
            && let Some(index) = declared.cached(self.generation)
            && let Some(slot) = self.hooks.get(index)
            && slot.name == declared.name()
        {
            return Some(index);
        }
        let name = hook.name();
        match self
            .hooks
            .binary_search_by(|slot| slot.name.as_str().cmp(name))
        {
            Ok(index) => {
                if let Some(declared) = hook.declared() {
                    declared.cache(self.generation, index);
                }
                Some(index)
            }
            Err(_) => None,
        }
    }

    /// The OR of every registered hook's filter bit.
    fn filter(&self) -> u64 {
        let mut bits = 0u64;
        for slot in &self.hooks {
            bits |= filter_bit(&slot.name);
        }
        bits
    }
}

/// One exported function.
#[derive(Clone)]
struct ApiEntry {
    id: u64,
    owner: &'static str,
    name: String,
    function: Arc<dyn ApiFn>,
}

/// Every function one plugin exports.
#[derive(Clone)]
struct ApiOwner {
    name: &'static str,
    functions: Vec<ApiEntry>,
}

/// The plugin function table, as one immutable snapshot.
#[derive(Clone)]
struct ApiTable {
    /// Sorted by owner name, so a lookup is a binary search.
    owners: Vec<ApiOwner>,
}

impl ApiTable {
    const fn new() -> Self {
        Self { owners: Vec::new() }
    }

    /// Whether `owner` already exports a live function called `name`.
    fn contains(&self, owner: &str, name: &str) -> bool {
        if let Ok(index) = self.owners.binary_search_by(|slot| slot.name.cmp(owner)) {
            for entry in &self.owners[index].functions {
                if entry.name == name {
                    return true;
                }
            }
        }
        false
    }

    fn insert(&mut self, entry: ApiEntry) {
        let index = match self
            .owners
            .binary_search_by(|slot| slot.name.cmp(entry.owner))
        {
            Ok(index) => index,
            Err(index) => {
                self.owners.insert(
                    index,
                    ApiOwner {
                        name: entry.owner,
                        functions: Vec::new(),
                    },
                );
                index
            }
        };
        self.owners[index].functions.push(entry);
    }

    /// Remove every function `predicate` accepts, dropping any owner left with none.
    fn take_matching(&mut self, predicate: impl Fn(&ApiEntry) -> bool) -> Vec<ApiEntry> {
        let mut removed = Vec::new();
        for slot in &mut self.owners {
            let mut kept = Vec::with_capacity(slot.functions.len());
            for entry in slot.functions.drain(..) {
                if predicate(&entry) {
                    removed.push(entry);
                } else {
                    kept.push(entry);
                }
            }
            slot.functions = kept;
        }
        self.owners.retain(|slot| !slot.functions.is_empty());
        removed
    }
}

/// The application's own functions, in one flat namespace with no plugin prefix.
#[derive(Clone)]
struct HostTable {
    functions: Vec<ApiEntry>,
}

impl HostTable {
    const fn new() -> Self {
        Self {
            functions: Vec::new(),
        }
    }
}

/// Which plugins exist and where each one is in its lifecycle.
#[derive(Clone)]
struct StateTable {
    plugins: Vec<(&'static str, State)>,
}

impl StateTable {
    const fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }
}

/// Everything one host owns, reached only through a [`Context`].
pub struct Tables {
    hooks: RwLock<Arc<HookTable>>,
    /// A 64-bit presence filter over registered hook names.
    ///
    /// This is the whole reason an unregistered hook costs almost nothing: `run` proves there is
    /// nothing to do with one relaxed load and a mask, never touching the lock or the `Arc`. False
    /// positives are harmless — they just fall through to the real lookup.
    filter: AtomicU64,
    apis: RwLock<Arc<ApiTable>>,
    hosts: RwLock<Arc<HostTable>>,
    states: RwLock<Arc<StateTable>>,
    next_id: AtomicU64,
}

impl Tables {
    /// Empty tables. A host leaks exactly one of these and hands out contexts pointing at it.
    pub(crate) fn new() -> Self {
        Self {
            hooks: RwLock::new(Arc::new(HookTable::new())),
            filter: AtomicU64::new(0),
            apis: RwLock::new(Arc::new(ApiTable::new())),
            hosts: RwLock::new(Arc::new(HostTable::new())),
            states: RwLock::new(Arc::new(StateTable::new())),
            next_id: AtomicU64::new(1),
        }
    }

    fn read_hooks(&self) -> RwLockReadGuard<'_, Arc<HookTable>> {
        match self.hooks.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_hooks(&self) -> RwLockWriteGuard<'_, Arc<HookTable>> {
        match self.hooks.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn read_apis(&self) -> RwLockReadGuard<'_, Arc<ApiTable>> {
        match self.apis.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_apis(&self) -> RwLockWriteGuard<'_, Arc<ApiTable>> {
        match self.apis.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    // ---- hooks ----------------------------------------------------------------------------

    /// Add a callback, tagged with `owner`.
    pub(crate) fn register(
        &self,
        owner: &'static str,
        hook: &str,
        priority: i32,
        callback: CallbackKind,
    ) -> RegistrationId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let label = callback.label();
        {
            let mut guard = self.write_hooks();
            let mut next = (**guard).clone();
            next.insert(
                hook,
                Entry {
                    id,
                    owner,
                    priority,
                    callback,
                },
            );
            *guard = Arc::new(next);
        }
        self.filter.fetch_or(filter_bit(hook), Ordering::Relaxed);

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::debug!(
                    msg = "Registered callback",
                    hook = hook,
                    kind = label,
                    priority = priority,
                    plugin = owner,
                    registration = id
                );
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Registered callback\" hook={hook} kind={label} priority={priority} \
                     plugin={owner} registration={id}"
                );
            } else {
                let _ = label;
            }
        }
        RegistrationId::new(id)
    }

    /// Remove one registration owned by `owner`. Returns whether anything matched.
    pub(crate) fn unregister(&self, owner: &str, registration: RegistrationId) -> bool {
        {
            let mut guard = self.write_hooks();
            let mut next = (**guard).clone();
            let removed =
                next.take_matching(|entry| entry.owner == owner && entry.id == registration.get());
            if removed.is_empty() {
                return false;
            }
            let filter = next.filter();
            *guard = Arc::new(next);
            self.filter.store(filter, Ordering::Relaxed);
            drop(guard);
            drop(removed);
        }
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::debug!(
                    msg = "Removed callback",
                    plugin = owner,
                    registration = registration.get()
                );
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Removed callback\" plugin={owner} registration={}",
                    registration.get()
                );
            }
        }
        true
    }

    /// Run every callback registered for `hook`, in priority order.
    ///
    /// The read lock is held only long enough to clone the snapshot `Arc`; callbacks run with no
    /// lock held, so one of them may register another, export a function, fire a nested hook or
    /// call into a second plugin without stalling anybody else.
    pub(crate) fn dispatch(
        &self,
        reach: Reach,
        hook: HookRef<'_>,
        data: &mut Value,
    ) -> Result<Flow> {
        if self.filter.load(Ordering::Relaxed) & hook.bit() == 0 {
            return Ok(Flow::Continue);
        }
        let snapshot = {
            let guard = self.read_hooks();
            Arc::clone(&guard)
        };
        let position = match snapshot.position_of(hook) {
            Some(position) => position,
            None => return Ok(Flow::Continue),
        };
        let entries = &snapshot.hooks[position].entries;
        let total = entries.len();
        if total == 0 {
            return Ok(Flow::Continue);
        }

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::trace_span!(
                    "hook.dispatch",
                    hook = hook.name(),
                    callback_count = total
                )
                .entered();
            } else if #[cfg(feature = "logging")] {
                log::trace!(
                    "msg=\"Dispatching hook\" hook={} callback_count={total}",
                    hook.name()
                );
            }
        }

        for entry in entries {
            let callee = Context::new(entry.owner, reach);
            let flow = match &entry.callback {
                CallbackKind::Transform(callback) => callback.call(&callee, data)?,
                CallbackKind::Observe(callback) => callback.call(&callee, data)?,
            };
            if flow == Flow::Stop {
                cfg_if! {
                    if #[cfg(feature = "tracing")] {
                        tracing::trace!(msg = "Callback stopped the dispatch", plugin = entry.owner);
                    } else if #[cfg(feature = "logging")] {
                        log::trace!(
                            "msg=\"Callback stopped the dispatch\" hook={} plugin={}",
                            hook.name(),
                            entry.owner
                        );
                    }
                }
                return Ok(Flow::Stop);
            }
        }
        Ok(Flow::Continue)
    }

    // ---- functions ------------------------------------------------------------------------

    /// Publish `function` under `owner`'s name, callable as `owner::name`.
    pub(crate) fn export(
        &self,
        owner: &'static str,
        name: &str,
        function: Arc<dyn ApiFn>,
    ) -> Result<RegistrationId> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut guard = self.write_apis();
            if guard.contains(owner, name) {
                return Err(Error::Duplicate {
                    plugin: owner.into(),
                    function: name.into(),
                });
            }
            let mut next = (**guard).clone();
            next.insert(ApiEntry {
                id,
                owner,
                name: name.to_string(),
                function,
            });
            *guard = Arc::new(next);
        }
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::debug!(msg = "Exported function", plugin = owner, function = name, registration = id);
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Exported function\" plugin={owner} function={name} registration={id}"
                );
            }
        }
        Ok(RegistrationId::new(id))
    }

    /// Withdraw one of `owner`'s functions. Returns whether it was there.
    pub(crate) fn unexport(&self, owner: &str, registration: RegistrationId) -> bool {
        {
            let mut guard = self.write_apis();
            let mut next = (**guard).clone();
            let removed =
                next.take_matching(|entry| entry.owner == owner && entry.id == registration.get());
            if removed.is_empty() {
                return false;
            }
            *guard = Arc::new(next);
            drop(guard);
            drop(removed);
        }
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::debug!(
                    msg = "Withdrew function",
                    plugin = owner,
                    registration = registration.get()
                );
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Withdrew function\" plugin={owner} registration={}",
                    registration.get()
                );
            }
        }
        true
    }

    /// Publish an application-side function under a flat name, with no plugin prefix.
    pub(crate) fn export_host(
        &self,
        name: &str,
        function: Arc<dyn ApiFn>,
    ) -> Result<RegistrationId> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut guard = match self.hosts.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            for entry in &guard.functions {
                if entry.name == name {
                    return Err(Error::Duplicate {
                        plugin: "host".into(),
                        function: name.into(),
                    });
                }
            }
            let mut next = (**guard).clone();
            next.functions.push(ApiEntry {
                id,
                owner: "host",
                name: name.to_string(),
                function,
            });
            *guard = Arc::new(next);
        }
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::debug!(msg = "Exported host function", function = name, registration = id);
            } else if #[cfg(feature = "logging")] {
                log::debug!("msg=\"Exported host function\" function={name} registration={id}");
            }
        }
        Ok(RegistrationId::new(id))
    }

    /// Call `plugin::function`, handing the callee its own context.
    pub(crate) fn plugin_call(&self, reach: Reach, target: &str, args: Value) -> Result<Value> {
        let (plugin, function) = match target.split_once("::") {
            Some((plugin, function)) if !plugin.is_empty() && !function.is_empty() => {
                (plugin, function)
            }
            _ => {
                return Err(Error::Malformed {
                    target: target.into(),
                });
            }
        };

        let snapshot = {
            let guard = self.read_apis();
            Arc::clone(&guard)
        };
        let mut found = None;
        if let Ok(index) = snapshot
            .owners
            .binary_search_by(|slot| slot.name.cmp(plugin))
        {
            for entry in &snapshot.owners[index].functions {
                if entry.name == function {
                    found = Some(entry);
                    break;
                }
            }
        }
        let entry = match found {
            Some(entry) => entry,
            None => return Err(self.why_missing(plugin, function)),
        };

        let callee = Context::new(entry.owner, reach);
        entry.function.call(&callee, args)
    }

    /// Why `plugin::function` did not resolve: a typo, an unstarted plugin, or a missing function.
    fn why_missing(&self, plugin: &str, function: &str) -> Error {
        let states = {
            let guard = match self.states.read() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            Arc::clone(&guard)
        };
        for (name, state) in &states.plugins {
            if *name == plugin {
                return match state {
                    State::Started => Error::NoSuchFunction {
                        plugin: plugin.into(),
                        function: function.into(),
                    },
                    _ => Error::NotStarted {
                        plugin: plugin.into(),
                    },
                };
            }
        }
        Error::NoSuchPlugin {
            plugin: plugin.into(),
        }
    }

    /// Call one of the application's own functions.
    pub(crate) fn host_call(&self, reach: Reach, name: &str, args: Value) -> Result<Value> {
        let snapshot = {
            let guard = match self.hosts.read() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            Arc::clone(&guard)
        };
        let mut found = None;
        for entry in &snapshot.functions {
            if entry.name == name {
                found = Some(entry);
                break;
            }
        }
        let entry = match found {
            Some(entry) => entry,
            None => {
                return Err(Error::NoSuchFunction {
                    plugin: "host".into(),
                    function: name.into(),
                });
            }
        };
        let callee = Context::new(entry.owner, reach);
        entry.function.call(&callee, args)
    }

    // ---- lifecycle ------------------------------------------------------------------------

    /// Record where `plugin` is in its lifecycle.
    pub(crate) fn set_state(&self, plugin: &'static str, state: State) {
        let mut guard = match self.states.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut next = (**guard).clone();
        let mut replaced = false;
        for slot in &mut next.plugins {
            if slot.0 == plugin {
                slot.1 = state;
                replaced = true;
                break;
            }
        }
        if !replaced {
            next.plugins.push((plugin, state));
        }
        *guard = Arc::new(next);
    }

    /// The leaked name this host knows `plugin` by, if it knows it at all.
    ///
    /// A plugin's name is leaked once, when it is loaded. Everything it registers afterwards — from
    /// inside a shared library, where the name arrives as borrowed bytes — is tagged with that same
    /// `&'static str`, so identity stays a pointer the tables already own.
    pub(crate) fn interned(&self, plugin: &str) -> Option<&'static str> {
        let guard = match self.states.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut interned = None;
        for (name, _) in &guard.plugins {
            if *name == plugin {
                interned = Some(*name);
                break;
            }
        }
        interned
    }

    /// Take every callback and every exported function owned by `owner` out of the tables.
    ///
    /// This is step one of the stop sequence: drain, quiesce ([`Retired::wait`]), drop, *then* stop
    /// the plugin. Calling the plugin's `stop` before the returned [`Retired`] has been waited on
    /// lets a callback or a function run against state the plugin has already torn down.
    pub(crate) fn retire(&self, owner: &'static str) -> Retired {
        let hooks_previous = {
            let mut guard = self.write_hooks();
            let mut next = (**guard).clone();
            let removed = next.take_matching(|entry| entry.owner == owner);
            let filter = next.filter();
            let previous = std::mem::replace(&mut *guard, Arc::new(next));
            self.filter.store(filter, Ordering::Relaxed);
            (previous, removed)
        };
        let apis_previous = {
            let mut guard = self.write_apis();
            let mut next = (**guard).clone();
            let removed = next.take_matching(|entry| entry.owner == owner);
            let previous = std::mem::replace(&mut *guard, Arc::new(next));
            (previous, removed)
        };

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::debug!(
                    msg = "Retired an owner from the tables",
                    plugin = owner,
                    callback_count = hooks_previous.1.len(),
                    function_count = apis_previous.1.len()
                );
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Retired an owner from the tables\" plugin={owner} \
                     callback_count={} function_count={}",
                    hooks_previous.1.len(),
                    apis_previous.1.len()
                );
            }
        }
        Retired {
            hooks: hooks_previous.0,
            apis: apis_previous.0,
            callbacks: hooks_previous.1,
            functions: apis_previous.1,
            owner,
        }
    }
}

/// Callbacks and functions taken out of the tables, waiting to be dropped.
///
/// This is the first half of a stop. They are already unreachable — a dispatch or a call starting
/// now cannot see them — but one that started *before* the swap may still be running. They are only
/// safe to drop once every such caller has finished, which is what [`Retired::wait`] establishes.
#[must_use = "the retired callbacks are not dropped until `wait` says it is safe"]
pub(crate) struct Retired {
    hooks: Arc<HookTable>,
    apis: Arc<ApiTable>,
    callbacks: Vec<Entry>,
    functions: Vec<ApiEntry>,
    owner: &'static str,
}

impl Retired {
    /// How many callbacks were taken out of the hook table.
    pub(crate) fn callbacks(&self) -> usize {
        self.callbacks.len()
    }

    /// How many exported functions were taken out of the function table.
    pub(crate) fn functions(&self) -> usize {
        self.functions.len()
    }

    /// Block until nothing can still be running one of these, then drop them.
    ///
    /// Returns [`Error::StopTimeout`] if `timeout` passes first — in which case **nothing is
    /// dropped**. They stay out of the tables, so the owner can receive nothing new and is harmless
    /// where it is; retry, or leave it drained.
    ///
    /// A caller must not call the owner's `stop` until this has returned `Ok`.
    pub(crate) fn wait(self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut backoff = Duration::from_micros(50);
        loop {
            // Nothing can clone these snapshots any more: they are out of the tables, so the counts
            // only fall. Reaching one means we hold the last reference and every dispatch or call
            // that could see this owner's entries has finished.
            let outstanding =
                (Arc::strong_count(&self.hooks) - 1) + (Arc::strong_count(&self.apis) - 1);
            if outstanding == 0 {
                break;
            }
            if Instant::now() >= deadline {
                cfg_if! {
                    if #[cfg(feature = "tracing")] {
                        tracing::warn!(
                            msg = "Timed out draining in-flight calls",
                            plugin = self.owner,
                            outstanding = outstanding
                        );
                    } else if #[cfg(feature = "logging")] {
                        log::warn!(
                            "msg=\"Timed out draining in-flight calls\" plugin={} \
                             outstanding={outstanding}",
                            self.owner
                        );
                    }
                }
                return Err(Error::StopTimeout {
                    outstanding: u32::try_from(outstanding).unwrap_or(u32::MAX),
                });
            }
            std::thread::sleep(backoff);
            backoff = std::cmp::min(backoff * 2, Duration::from_millis(5));
        }
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::debug!(
                    msg = "Drained an owner",
                    plugin = self.owner,
                    callback_count = self.callbacks.len(),
                    function_count = self.functions.len()
                );
            } else if #[cfg(feature = "logging")] {
                log::debug!(
                    "msg=\"Drained an owner\" plugin={} callback_count={} function_count={}",
                    self.owner,
                    self.callbacks.len(),
                    self.functions.len()
                );
            }
        }
        // Dropping here runs each callback's and each function's destructor. For a loaded plugin
        // that code lives in its library, which is still mapped — plugx never unloads one.
        drop(self);
        Ok(())
    }
}
