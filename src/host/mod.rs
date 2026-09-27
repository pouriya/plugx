//! # Host
//!
//! The application side: own the registry, find plugins, load them, order them by what they depend
//! on, start them, reconfigure them, and stop them safely.
//!
//! ```rust
//! use plugx::Host;
//!
//! let mut host = Host::new()?;
//! # let _ = &mut host;
//! // host.export("now", |_: &plugx::Context, _args| Ok(plugx::Value::Int(0)))?;
//! // host.add_dir("plugins");
//! // host.load_all()?;
//! // host.start_all(&configs)?;
//! # Ok::<(), plugx::host::Error>(())
//! ```
//!
//! # The host owns the registry
//!
//! A [`Host`] allocates one registry, leaks it, and puts it in this program's one slot.
//! Everything reads them from there: [`plugx::run`](crate::run) for code linked into the
//! application, and the [`Context`] each plugin is handed for code inside a loaded library.
//!
//! One slot means one live host. [`Host::new`] refuses while another is alive and gives the slot
//! back when it is dropped.
//!
//! # Stopping is an ordered sequence, and the order is the point
//!
//! ```text
//!   1. drain    — take the plugin's callbacks and exported functions out of the registry
//!   2. quiesce  — wait for dispatches and calls that started before step 1 to finish
//!   3. drop     — free them, while the plugin's code is still mapped
//!   4. stop     — only now tell the plugin to tear its own state down
//!   5. leave the library mapped, forever
//! ```
//!
//! Doing step 4 before step 2 is the mistake this crate exists to prevent: a callback that is
//! already running would be working against state the plugin has just torn down. Nothing in plugx
//! can detect that afterwards, so the order is enforced here instead.
//!
//! Only the thread calling [`Host::stop`] waits. Threads dispatching hooks are never blocked, and a
//! dispatch already in flight runs to completion.

#[cfg(feature = "config-tanzim")]
/// Validating a plugin's configuration against the spec it declared.
pub mod config;
/// [`Error`] and the crate's [`Result`] alias.
pub mod error;

pub use error::{Error, Result};

pub use crate::registry::State;

use crate::context::Context;
use crate::plugin::load::{Loader, split_scheme};
use crate::plugin::runtime::Runtime;
use crate::plugin::{Info, Plugin};
use crate::registry::{ApiFn, Registry};
use crate::value::Value;
use cfg_if::cfg_if;
use std::time::Duration;

/// How long [`Host::stop`] waits for in-flight dispatches before giving up.
pub const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The namespace the application's own registrations and functions are filed under.
const HOST_NAMESPACE: &str = "host";

/// The name a plugin is asked for its [`Info`] under, before it has reported one.
const UNNAMED: &str = "";

/// One plugin the host is looking after.
struct Managed {
    name: &'static str,
    plugin: Box<dyn Plugin>,
    info: Info,
    config: Value,
    state: State,
}

/// The application's plugin runtime.
pub struct Host {
    loader_list: Vec<Box<dyn Loader>>,
    runtime_list: Vec<Box<dyn Runtime>>,
    plugin_list: Vec<Managed>,
    source_list: Vec<String>,
    registry: &'static Registry,
    stop_timeout: Duration,
}

impl Drop for Host {
    /// Gives this program's slot back, so a later host can claim it.
    ///
    /// The registry itself stays leaked: a plugin library is never unloaded, and code inside one
    /// may still hold the pointer.
    fn drop(&mut self) {
        crate::registry::uninstall(self.registry);
    }
}

impl Host {
    /// A host with every feature-enabled loader and runtime already registered.
    ///
    /// Claims this program's one slot, and fails with [`Error::HostExists`] if another host is
    /// still alive. Allocates this host's registry and leaks it: a few hundred bytes, once, for
    /// the life of the process — which is what lets a plugin keep its [`Context`] on a background
    /// thread without any lifetime crossing the ABI.
    pub fn new() -> Result<Self> {
        #[allow(unused_mut)]
        let mut host = Self::empty()?;
        #[cfg(feature = "load-file")]
        {
            host = host.with_loader(crate::plugin::load::file::File::new());
        }
        #[cfg(feature = "runtime-cdylib")]
        {
            host = host.with_runtime(crate::plugin::runtime::cdylib::Cdylib::new());
        }
        Ok(host)
    }

    /// A host with no loaders and no runtimes at all. Add them with
    /// [`with_loader`](Self::with_loader) and [`with_runtime`](Self::with_runtime).
    ///
    /// Claims the slot exactly as [`new`](Self::new) does.
    pub fn empty() -> Result<Self> {
        let registry: &'static Registry = Box::leak(Box::new(Registry::new()));
        if !crate::registry::install(registry) {
            return Err(Error::HostExists);
        }
        Ok(Self {
            loader_list: Vec::new(),
            runtime_list: Vec::new(),
            plugin_list: Vec::new(),
            source_list: Vec::new(),
            registry,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
        })
    }

    /// Add somewhere plugins are to be fetched from, as `scheme://rest` or as a plain path.
    ///
    /// ```text
    ///   plugins                    every file in that directory, relative to the process
    ///   ./build/libauth.so         that one file
    ///   /opt/plugins               every file in that directory
    ///   file://plugins             the same thing, said in full
    ///   https://cdn/plugins.json   whatever an HTTP loader makes of it
    /// ```
    ///
    /// **A source with no `://` is a path**, which is to say a `file` source: that is what anyone
    /// types first, and there is nothing else it could sensibly mean. A Windows path is one too —
    /// neither `C:\plugins` nor `C:/plugins` contains `://`.
    ///
    /// The scheme is checked here, against the loaders this host has: a source no loader claims is
    /// [`Error::Load`] now rather than a surprise at [`load_all`](Self::load_all). Nothing is
    /// fetched yet.
    pub fn add_plugin_source(&mut self, source: &str) -> Result<()> {
        let (scheme, _) = split_scheme(source);
        if self.loader_for(scheme).is_none() {
            return Err(Error::from(crate::plugin::load::Error::UnknownScheme {
                source: source.into(),
            }));
        }
        self.source_list.push(source.to_string());
        Ok(())
    }

    /// Fetch every source added with [`add_plugin_source`](Self::add_plugin_source), and build
    /// every artifact that comes back.
    ///
    /// Each source goes to the loader claiming its scheme, and each artifact it yields goes to the
    /// first runtime claiming its extension. An artifact no runtime claims is skipped in silence,
    /// because a place plugins are fetched from is allowed to hold other things.
    ///
    /// A plugin's name is its identity, so a second plugin under a name already taken is
    /// [`Error::Duplicate`]. Note that the artifact has already been built by then — a colliding
    /// shared library is opened, and stays mapped, because plugx never unloads one.
    ///
    /// Loading does not start anything: no callbacks are registered, nothing is exported, and no
    /// configuration is needed yet.
    pub fn load_all(&mut self) -> Result<()> {
        let source_list = std::mem::take(&mut self.source_list);
        for source in &source_list {
            let (scheme, _) = split_scheme(source);
            let loader = match self.loader_for(scheme) {
                Some(loader) => loader,
                None => continue,
            };

            cfg_if! {
                if #[cfg(feature = "tracing")] {
                    let _span = tracing::debug_span!(
                        "plugin.fetch",
                        loader = loader.name(),
                        source = source.as_str()
                    )
                    .entered();
                } else if #[cfg(feature = "logging")] {
                    log::debug!(
                        "msg=\"Fetching plugin source\" loader={} source={source}",
                        loader.name()
                    );
                }
            }

            let artifact_list = loader.load(source)?;
            for artifact in artifact_list {
                let runtime = match self.runtime_for(&artifact.name) {
                    Some(runtime) => runtime,
                    None => continue,
                };
                // Copied out because `adopt` needs `&mut self` and these still borrow it.
                let runtime_name = runtime.name().to_string();
                let from = artifact.source.clone();
                let plugin_list = runtime.build(artifact, self.registry)?;
                for plugin in plugin_list {
                    self.adopt(plugin, &runtime_name, &from)?;
                }
            }
        }
        Ok(())
    }

    /// Take a freshly built plugin into this host: ask what it is, and adopt it under the name it
    /// reports.
    ///
    /// The context this is asked through is [`UNNAMED`], because the name is exactly what is being
    /// asked for. A plugin does not answer out of that context — it answers out of what its
    /// runtime gave it — so a runtime whose plugins reach the host during `info` must carry their
    /// own name, as every runtime must anyway to fill [`Info::name`].
    fn adopt(&mut self, plugin: Box<dyn Plugin>, runtime: &str, from: &str) -> Result<()> {
        let info = match plugin.info(&Context::direct(UNNAMED, self.registry)) {
            Ok(info) => info,
            Err(source) => {
                return Err(Error::Plugin {
                    plugin: from.into(),
                    source: Box::new(source),
                });
            }
        };

        let name = info.name;
        if name.is_empty() {
            return Err(Error::from(crate::plugin::runtime::Error::Unnamed {
                source: from.into(),
            }));
        }
        if self.index_of(name).is_some() {
            return Err(Error::Duplicate {
                plugin: name.into(),
            });
        }

        // Announced before anything else can reach it, so a plugin that calls back into the host
        // is already addressable — as `NotStarted`, which is the truth.
        self.registry.set_state(name, State::Loaded);

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::info!(
                    msg = "Loaded plugin",
                    plugin = name,
                    runtime = runtime,
                    version = %info.version
                );
            } else if #[cfg(feature = "logging")] {
                log::info!(
                    "msg=\"Loaded plugin\" plugin={name} runtime={runtime} version={}",
                    info.version
                );
            } else {
                let _ = runtime;
            }
        }

        self.plugin_list.push(Managed {
            name,
            plugin,
            info,
            config: Value::map(),
            state: State::Loaded,
        });
        Ok(())
    }

    /// Add a loader: somewhere plugins can be fetched from. Tried in the order they were added.
    pub fn with_loader(mut self, loader: impl Loader + 'static) -> Self {
        self.loader_list.push(Box::new(loader));
        self
    }

    /// Add a runtime: something artifacts can be run as. Tried in the order they were added.
    pub fn with_runtime(mut self, runtime: impl Runtime + 'static) -> Self {
        self.runtime_list.push(Box::new(runtime));
        self
    }

    /// How long [`stop`](Self::stop) waits for in-flight dispatches before returning
    /// [`Error::Drain`].
    pub fn with_stop_timeout(mut self, timeout: Duration) -> Self {
        self.stop_timeout = timeout;
        self
    }

    /// The application's own context.
    ///
    /// Hand it to anything of yours that fires hooks, and use it to register the application's own
    /// callbacks. Registrations made through it land in the `host` namespace and are never drained.
    pub const fn context(&self) -> Context {
        Context::direct(HOST_NAMESPACE, self.registry)
    }

    /// Publish one of the application's functions, callable by any plugin as
    /// [`host_call`](Context::host_call). Flat names, no plugin prefix.
    pub fn export(
        &self,
        name: &str,
        function: impl ApiFn + 'static,
    ) -> crate::Result<crate::RegistrationId> {
        self.registry
            .export_host(name, std::sync::Arc::new(function))
    }

    /// Every plugin the host knows about, with its state.
    pub fn plugin_list(&self) -> impl Iterator<Item = (&str, State)> {
        self.plugin_list
            .iter()
            .map(|managed| (managed.name, managed.state))
    }

    /// What a loaded plugin reported about itself.
    pub fn info(&self, name: &str) -> Option<&Info> {
        let managed = self.find(name)?;
        Some(&managed.info)
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        for (index, managed) in self.plugin_list.iter().enumerate() {
            if managed.name == name {
                return Some(index);
            }
        }
        None
    }

    /// The first loader that claims this scheme.
    fn loader_for(&self, scheme: &str) -> Option<&dyn Loader> {
        for loader in &self.loader_list {
            for candidate in loader.schema_list() {
                if scheme.eq_ignore_ascii_case(candidate) {
                    return Some(loader.as_ref());
                }
            }
        }
        None
    }

    /// The first runtime that claims this artifact name's extension.
    fn runtime_for(&self, name: &str) -> Option<&dyn Runtime> {
        let extension = match name.rsplit_once('.') {
            Some((_, extension)) => extension,
            None => return None,
        };
        for runtime in &self.runtime_list {
            for candidate in runtime.extension_list() {
                if extension.eq_ignore_ascii_case(candidate) {
                    return Some(runtime.as_ref());
                }
            }
        }
        None
    }

    fn find(&self, name: &str) -> Option<&Managed> {
        let index = self.index_of(name)?;
        Some(&self.plugin_list[index])
    }

    fn position(&self, name: &str) -> Result<usize> {
        match self.index_of(name) {
            Some(index) => Ok(index),
            None => Err(Error::Unknown {
                plugin: name.into(),
            }),
        }
    }

    /// Start one plugin with `config`.
    ///
    /// Its dependencies must already be started. The plugin registers its hook callbacks and
    /// exports its functions during this call, all in its own namespace.
    pub fn start(&mut self, name: &str, config: Value) -> Result<()> {
        let index = self.position(name)?;
        if self.plugin_list[index].state == State::Started {
            return Err(Error::WrongState {
                plugin: name.into(),
                state: State::Started.label(),
                expected: "loaded or stopped",
            });
        }
        self.check_dependencies(index)?;
        let config = self.validate(index, config)?;

        let registry = self.registry;
        let managed = &mut self.plugin_list[index];
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::info_span!("plugin.start", plugin = managed.name).entered();
                tracing::debug!(msg = "Starting plugin");
            } else if #[cfg(feature = "logging")] {
                log::debug!("msg=\"Starting plugin\" plugin={}", managed.name);
            }
        }

        // Announced before the call, not after: a plugin's `start` may export a function and then
        // fire a hook whose callbacks call it straight back.
        registry.set_state(managed.name, State::Started);
        let context = Context::direct(managed.name, registry);
        match managed.plugin.start(&context, &config) {
            Ok(()) => {
                managed.config = config;
                managed.state = State::Started;
                cfg_if! {
                    if #[cfg(feature = "tracing")] {
                        tracing::info!(msg = "Started plugin");
                    } else if #[cfg(feature = "logging")] {
                        log::info!("msg=\"Started plugin\" plugin={}", managed.name);
                    }
                }
                Ok(())
            }
            Err(source) => {
                registry.set_state(managed.name, State::Loaded);
                Err(Error::Plugin {
                    plugin: managed.name.into(),
                    source: Box::new(source),
                })
            }
        }
    }

    /// Hand a started plugin a new configuration.
    ///
    /// This is a **configuration** reload: the library stays mapped, and the plugin's registrations
    /// and exports stay in place unless it changes them itself. A plugin that does not implement
    /// `reload` gets stopped and started again instead.
    pub fn reload(&mut self, name: &str, config: Value) -> Result<()> {
        let index = self.position(name)?;
        if self.plugin_list[index].state != State::Started {
            return Err(Error::WrongState {
                plugin: name.into(),
                state: self.plugin_list[index].state.label(),
                expected: "started",
            });
        }
        let config = self.validate(index, config)?;

        let registry = self.registry;
        let managed = &mut self.plugin_list[index];
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::info_span!("plugin.reload", plugin = managed.name).entered();
                tracing::debug!(msg = "Reloading plugin configuration");
            } else if #[cfg(feature = "logging")] {
                log::debug!("msg=\"Reloading plugin configuration\" plugin={}", managed.name);
            }
        }

        let old = managed.config.clone();
        let context = Context::direct(managed.name, registry);
        match managed.plugin.reload(&context, &old, &config) {
            Ok(()) => {
                managed.config = config;
                cfg_if! {
                    if #[cfg(feature = "tracing")] {
                        tracing::info!(msg = "Reloaded plugin configuration");
                    } else if #[cfg(feature = "logging")] {
                        log::info!(
                            "msg=\"Reloaded plugin configuration\" plugin={}",
                            managed.name
                        );
                    }
                }
                Ok(())
            }
            Err(crate::plugin::Error::Unsupported { .. }) => {
                cfg_if! {
                    if #[cfg(feature = "tracing")] {
                        tracing::warn!(
                            msg = "Plugin does not support reload, restarting it instead"
                        );
                    } else if #[cfg(feature = "logging")] {
                        log::warn!(
                            "msg=\"Plugin does not support reload, restarting it instead\" \
                             plugin={}",
                            managed.name
                        );
                    }
                }
                let name = managed.name;
                self.stop(name)?;
                self.start(name, config)
            }
            Err(source) => Err(Error::Plugin {
                plugin: managed.name.into(),
                source: Box::new(source),
            }),
        }
    }

    /// Stop one plugin.
    ///
    /// Drains its callbacks and its exported functions, waits for every dispatch and call that
    /// could still reach them, drops them, and only then calls the plugin's own `stop`. The library
    /// stays mapped.
    ///
    /// Returns [`Error::Drain`] if something was still running when the stop timeout passed. The
    /// plugin is then drained but not stopped — harmless where it is, and safe to retry.
    pub fn stop(&mut self, name: &str) -> Result<()> {
        let index = self.position(name)?;
        if self.plugin_list[index].state != State::Started {
            return Err(Error::WrongState {
                plugin: name.into(),
                state: self.plugin_list[index].state.label(),
                expected: "started",
            });
        }
        let registry = self.registry;
        let stop_timeout = self.stop_timeout;
        let managed = &mut self.plugin_list[index];

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::info_span!("plugin.stop", plugin = managed.name).entered();
            } else if #[cfg(feature = "logging")] {
                log::debug!("msg=\"Stopping plugin\" plugin={}", managed.name);
            }
        }

        // 1. Take the callbacks and the exported functions out of the registry. A dispatch or a call
        //    starting now cannot see them.
        let drained = registry.drain(managed.name);
        let callback_count = drained.callback_count();
        let function_count = drained.function_count();

        // 2 and 3. Wait for dispatches and calls that started *before* step 1, then drop what was
        //    removed — while the plugin's library is still mapped, which it always is.
        if let Err(source) = drained.wait(stop_timeout) {
            return Err(Error::Drain {
                plugin: managed.name.into(),
                source: Box::new(source),
            });
        }
        registry.set_state(managed.name, State::Stopped);

        // 4. Only now can the plugin safely tear down what those callbacks were using.
        let context = Context::direct(managed.name, registry);
        match managed.plugin.stop(&context) {
            Ok(()) => {
                managed.state = State::Stopped;
                cfg_if! {
                    if #[cfg(feature = "tracing")] {
                        tracing::info!(
                            msg = "Stopped plugin",
                            callback_count = callback_count,
                            function_count = function_count
                        );
                    } else if #[cfg(feature = "logging")] {
                        log::info!(
                            "msg=\"Stopped plugin\" plugin={} callback_count={callback_count} \
                             function_count={function_count}",
                            managed.name
                        );
                    } else {
                        let _ = (callback_count, function_count);
                    }
                }
                Ok(())
            }
            Err(source) => {
                // Everything it registered is gone either way, so the plugin can do no more harm.
                managed.state = State::Stopped;
                Err(Error::Plugin {
                    plugin: managed.name.into(),
                    source: Box::new(source),
                })
            }
        }
    }

    /// Start every loaded plugin, in an order that satisfies their declared dependencies.
    ///
    /// `configs` supplies each plugin's configuration by name; a plugin with no entry is started
    /// with an empty map.
    pub fn start_all(&mut self, configs: &crate::value::Map) -> Result<()> {
        let order_list = self.start_order()?;
        for name in order_list {
            let config = match configs.get(name) {
                Some(config) => config.clone(),
                None => Value::map(),
            };
            self.start(name, config)?;
        }
        Ok(())
    }

    /// Stop every started plugin, in the reverse of the order they were started in.
    ///
    /// Keeps going after a failure so that one stuck plugin does not strand the rest, and reports
    /// the first error at the end.
    pub fn stop_all(&mut self) -> Result<()> {
        let mut order_list = self.start_order()?;
        order_list.reverse();
        let mut first_error = None;
        for name in order_list {
            let started = match self.find(name) {
                Some(managed) => managed.state == State::Started,
                None => false,
            };
            if !started {
                continue;
            }
            match self.stop(name) {
                Ok(()) => {}
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The order in which the loaded plugins can be started so that every dependency comes first.
    ///
    /// A plain Kahn's algorithm over the declared dependencies. Optional dependencies are ordered
    /// when present and skipped when not.
    pub fn start_order(&self) -> Result<Vec<&'static str>> {
        let mut remaining_list: Vec<&Managed> = self.plugin_list.iter().collect();
        let mut order_list: Vec<&'static str> = Vec::with_capacity(remaining_list.len());
        while !remaining_list.is_empty() {
            let mut ready_list = Vec::new();
            for managed in &remaining_list {
                let mut satisfied = true;
                for dependency in &managed.info.dependency_list {
                    let present = self.find(&dependency.name).is_some();
                    if !present {
                        continue;
                    }
                    let mut placed = false;
                    for name in &order_list {
                        if *name == dependency.name {
                            placed = true;
                            break;
                        }
                    }
                    if !placed {
                        satisfied = false;
                        break;
                    }
                }
                if satisfied {
                    ready_list.push(managed.name);
                }
            }
            if ready_list.is_empty() {
                let mut stuck_list = Vec::with_capacity(remaining_list.len());
                for managed in &remaining_list {
                    stuck_list.push(Box::from(managed.name));
                }
                return Err(Error::Cycle {
                    plugin_list: stuck_list.into_boxed_slice(),
                });
            }
            remaining_list.retain(|managed| !ready_list.contains(&managed.name));
            order_list.extend(ready_list);
        }
        Ok(order_list)
    }

    /// Check `config` against the plugin's declared spec, coercing it where the spec allows.
    ///
    /// Without the `config-tanzim` feature there is no validator to check against, so the
    /// configuration is passed through as the plugin declared it should be shaped — the plugin
    /// still gets to reject it from `start`.
    #[cfg(feature = "config-tanzim")]
    fn validate(&self, index: usize, config: Value) -> Result<Value> {
        let managed = &self.plugin_list[index];
        let schema = match managed.info.config_spec.schema() {
            Some(schema) => schema,
            None => return Ok(config),
        };
        match config::validate(schema, config) {
            Ok(config) => Ok(config),
            Err(message) => Err(Error::Config {
                plugin: managed.name.into(),
                message,
            }),
        }
    }

    #[cfg(not(feature = "config-tanzim"))]
    fn validate(&self, _index: usize, config: Value) -> Result<Value> {
        Ok(config)
    }

    /// Check that every declared dependency is loaded at an acceptable version.
    fn check_dependencies(&self, index: usize) -> Result<()> {
        let managed = &self.plugin_list[index];
        for dependency in &managed.info.dependency_list {
            let found = self.find(&dependency.name);
            let mut version = None;
            if let Some(other) = found {
                version = Some(other.info.version);
            }
            let acceptable = match version {
                Some(version) => dependency.accepts(version),
                None => dependency.optional,
            };
            if !acceptable {
                return Err(Error::Dependency {
                    plugin: managed.name.into(),
                    needs: format!("`{}` >= {}", dependency.name, dependency.minimum)
                        .into_boxed_str(),
                    found: version,
                });
            }
        }
        Ok(())
    }
}
