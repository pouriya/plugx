//! # Host
//!
//! The application side: own the tables, find plugins, load them, order them by what they depend
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
//! # The host owns the tables
//!
//! A [`Host`] allocates one set of tables, leaks them, and puts them in this program's one slot.
//! Everything reads them from there: [`plugx::run`](crate::run) for code linked into the
//! application, and the [`Context`] each plugin is handed for code inside a loaded library.
//!
//! One slot means one live host. [`Host::new`] refuses while another is alive and gives the slot
//! back when it is dropped.
//!
//! # Stopping is an ordered sequence, and the order is the point
//!
//! ```text
//!   1. retire   — take the plugin's callbacks and exported functions out of the tables
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

pub use crate::tables::State;

use crate::context::Context;
use crate::load::{Loader, Request};
use crate::plugin::{Info, Plugin};
use crate::tables::{ApiFn, Tables};
use crate::value::Value;
use cfg_if::cfg_if;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long [`Host::stop`] waits for in-flight dispatches before giving up.
pub const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The name the application's own registrations and functions are tagged with.
const HOST_NAME: &str = "host";

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
    loaders: Vec<Box<dyn Loader>>,
    plugins: Vec<Managed>,
    directories: Vec<PathBuf>,
    tables: &'static Tables,
    stop_timeout: Duration,
}

impl Drop for Host {
    /// Gives this program's slot back, so a later host can claim it.
    ///
    /// The tables themselves stay leaked: a plugin library is never unloaded, and code inside one
    /// may still hold the pointer.
    fn drop(&mut self) {
        crate::global::uninstall(self.tables);
    }
}

impl Host {
    /// A host with every feature-enabled loader already registered.
    ///
    /// Claims this program's one slot, and fails with [`Error::HostExists`] if another host is
    /// still alive. Allocates this host's tables and leaks them: a few hundred bytes, once, for
    /// the life of the process — which is what lets a plugin keep its [`Context`] on a background
    /// thread without any lifetime crossing the ABI.
    pub fn new() -> Result<Self> {
        #[allow(unused_mut)]
        let mut host = Self::empty()?;
        #[cfg(feature = "load-cdylib")]
        {
            host = host.with_loader(crate::load::cdylib::Cdylib::new());
        }
        Ok(host)
    }

    /// A host with no loaders at all. Add them with [`with_loader`](Self::with_loader).
    ///
    /// Claims the slot exactly as [`new`](Self::new) does.
    pub fn empty() -> Result<Self> {
        let tables: &'static Tables = Box::leak(Box::new(Tables::new()));
        if !crate::global::install(tables) {
            return Err(Error::HostExists);
        }
        Ok(Self {
            loaders: Vec::new(),
            plugins: Vec::new(),
            directories: Vec::new(),
            tables,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
        })
    }

    /// Add a directory to scan for plugins when [`load_all`](Self::load_all) runs.
    pub fn add_dir(&mut self, directory: impl AsRef<Path>) {
        self.directories.push(directory.as_ref().to_path_buf());
    }

    /// Load every plugin in every directory added with [`add_dir`](Self::add_dir).
    ///
    /// A file goes to the first loader that claims its extension; a file no loader claims is
    /// skipped in silence, because a plugin directory is allowed to hold other things.
    pub fn load_all(&mut self) -> Result<()> {
        let directories = self.directories.clone();
        for directory in &directories {
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) => {
                    return Err(Error::Directory {
                        path: directory.display().to_string().into_boxed_str(),
                        message: error.to_string().into_boxed_str(),
                    });
                }
            };
            let mut paths = Vec::new();
            for entry in entries {
                match entry {
                    Ok(entry) => paths.push(entry.path()),
                    Err(error) => {
                        return Err(Error::Directory {
                            path: directory.display().to_string().into_boxed_str(),
                            message: error.to_string().into_boxed_str(),
                        });
                    }
                }
            }
            paths.sort();
            for path in &paths {
                if self.loader_for(path).is_none() {
                    continue;
                }
                self.load(path)?;
            }
        }
        Ok(())
    }

    /// Add a loader. They are tried in the order they were added.
    pub fn with_loader(mut self, loader: impl Loader + 'static) -> Self {
        self.loaders.push(Box::new(loader));
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
    /// callbacks. Registrations made through it are tagged `host` and are never retired.
    pub const fn context(&self) -> Context {
        Context::local(HOST_NAME, self.tables)
    }

    /// Publish one of the application's functions, callable by any plugin as
    /// [`host_call`](Context::host_call). Flat names, no plugin prefix.
    pub fn export(
        &self,
        name: &str,
        function: impl ApiFn + 'static,
    ) -> crate::Result<crate::RegistrationId> {
        self.tables.export_host(name, std::sync::Arc::new(function))
    }

    /// Every plugin the host knows about, with its state.
    pub fn plugins(&self) -> impl Iterator<Item = (&'static str, State)> {
        self.plugins
            .iter()
            .map(|managed| (managed.name, managed.state))
    }

    /// What a loaded plugin reported about itself.
    pub fn info(&self, name: &str) -> Option<&Info> {
        let managed = self.find(name)?;
        Some(&managed.info)
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        for (index, managed) in self.plugins.iter().enumerate() {
            if managed.name == name {
                return Some(index);
            }
        }
        None
    }

    /// The first loader that claims this path's extension.
    fn loader_for(&self, path: &Path) -> Option<&dyn Loader> {
        let extension = path.extension()?;
        for loader in &self.loaders {
            for candidate in loader.extension_list() {
                if extension.eq_ignore_ascii_case(candidate) {
                    return Some(loader.as_ref());
                }
            }
        }
        None
    }

    fn find(&self, name: &str) -> Option<&Managed> {
        let index = self.index_of(name)?;
        Some(&self.plugins[index])
    }

    fn position(&self, name: &str) -> Result<usize> {
        match self.index_of(name) {
            Some(index) => Ok(index),
            None => Err(Error::Unknown {
                plugin: name.into(),
            }),
        }
    }

    /// Load `path` as a plugin, and ask it what it is.
    ///
    /// The plugin's name is its filename with any `lib` prefix and extension removed, so
    /// `plugins/libauth.so` loads as `auth` on every platform. The name is its identity — what its
    /// registrations are tagged with and how other plugins address its functions — so a second
    /// plugin under a name already taken is [`Error::Duplicate`]. It is leaked here, once, and
    /// stays valid for the life of the process.
    ///
    /// Loading does not start anything: no callbacks are registered, nothing is exported, and no
    /// configuration is needed yet.
    pub fn load(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let stem = match path.file_stem() {
            Some(stem) => stem.to_string_lossy(),
            None => {
                return Err(Error::Unnamed {
                    path: path.display().to_string().into_boxed_str(),
                });
            }
        };
        let name = match stem.strip_prefix("lib") {
            Some(rest) => rest,
            None => stem.as_ref(),
        };
        if name.is_empty() {
            return Err(Error::Unnamed {
                path: path.display().to_string().into_boxed_str(),
            });
        }
        if self.index_of(name).is_some() {
            return Err(Error::Duplicate {
                plugin: name.into(),
            });
        }

        let loader = match self.loader_for(path) {
            Some(loader) => loader,
            None => {
                return Err(Error::from(crate::load::Error::Unsupported {
                    path: path.to_path_buf(),
                }));
            }
        };

        // Leaked once, here. Everything this plugin registers is tagged with this exact `&'static
        // str`, so identity is a pointer the tables already own rather than a string compared over
        // and over — and a plugin running inside a shared library can hold on to it for good.
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());
        let request = Request::new(name, path, self.tables);
        let plugin = loader.load(&request)?;

        // Announced before `info`, so a plugin that calls back into the host during its own
        // inspection is already addressable — as `NotStarted`, which is the truth.
        self.tables.set_state(name, State::Loaded);

        let info = match plugin.info(&Context::local(name, self.tables)) {
            Ok(info) => info,
            Err(source) => {
                return Err(Error::Plugin {
                    plugin: name.into(),
                    source: Box::new(source),
                });
            }
        };

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::info!(
                    msg = "Loaded plugin",
                    plugin = name,
                    loader = loader.name(),
                    version = %info.version
                );
            } else if #[cfg(feature = "logging")] {
                log::info!(
                    "msg=\"Loaded plugin\" plugin={name} loader={} version={}",
                    loader.name(),
                    info.version
                );
            }
        }

        self.plugins.push(Managed {
            name,
            plugin,
            info,
            config: Value::map(),
            state: State::Loaded,
        });
        Ok(())
    }

    /// Start one plugin with `config`.
    ///
    /// Its dependencies must already be started. The plugin registers its hook callbacks and
    /// exports its functions during this call, all tagged with its own name.
    pub fn start(&mut self, name: &str, config: Value) -> Result<()> {
        let index = self.position(name)?;
        if self.plugins[index].state == State::Started {
            return Err(Error::WrongState {
                plugin: name.into(),
                state: State::Started.label(),
                expected: "loaded or stopped",
            });
        }
        self.check_dependencies(index)?;
        let config = self.validate(index, config)?;

        let tables = self.tables;
        let managed = &mut self.plugins[index];
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
        tables.set_state(managed.name, State::Started);
        let context = Context::local(managed.name, tables);
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
                tables.set_state(managed.name, State::Loaded);
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
        if self.plugins[index].state != State::Started {
            return Err(Error::WrongState {
                plugin: name.into(),
                state: self.plugins[index].state.label(),
                expected: "started",
            });
        }
        let config = self.validate(index, config)?;

        let tables = self.tables;
        let managed = &mut self.plugins[index];
        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::info_span!("plugin.reload", plugin = managed.name).entered();
                tracing::debug!(msg = "Reloading plugin configuration");
            } else if #[cfg(feature = "logging")] {
                log::debug!("msg=\"Reloading plugin configuration\" plugin={}", managed.name);
            }
        }

        let old = managed.config.clone();
        let context = Context::local(managed.name, tables);
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
        if self.plugins[index].state != State::Started {
            return Err(Error::WrongState {
                plugin: name.into(),
                state: self.plugins[index].state.label(),
                expected: "started",
            });
        }
        let tables = self.tables;
        let stop_timeout = self.stop_timeout;
        let managed = &mut self.plugins[index];

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::info_span!("plugin.stop", plugin = managed.name).entered();
            } else if #[cfg(feature = "logging")] {
                log::debug!("msg=\"Stopping plugin\" plugin={}", managed.name);
            }
        }

        // 1. Take the callbacks and the exported functions out of the tables. A dispatch or a call
        //    starting now cannot see them.
        let retired = tables.retire(managed.name);
        let callback_count = retired.callbacks();
        let function_count = retired.functions();

        // 2 and 3. Wait for dispatches and calls that started *before* step 1, then drop what was
        //    removed — while the plugin's library is still mapped, which it always is.
        if let Err(source) = retired.wait(stop_timeout) {
            return Err(Error::Drain {
                plugin: managed.name.into(),
                source: Box::new(source),
            });
        }
        tables.set_state(managed.name, State::Stopped);

        // 4. Only now can the plugin safely tear down what those callbacks were using.
        let context = Context::local(managed.name, tables);
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
        let order = self.start_order()?;
        for name in order {
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
        let mut order = self.start_order()?;
        order.reverse();
        let mut first_error = None;
        for name in order {
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
        let mut remaining: Vec<&Managed> = self.plugins.iter().collect();
        let mut order: Vec<&'static str> = Vec::with_capacity(remaining.len());
        while !remaining.is_empty() {
            let mut ready = Vec::new();
            for managed in &remaining {
                let mut satisfied = true;
                for dependency in &managed.info.dependencies {
                    let present = self.find(&dependency.name).is_some();
                    if !present {
                        continue;
                    }
                    let mut placed = false;
                    for name in &order {
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
                    ready.push(managed.name);
                }
            }
            if ready.is_empty() {
                let mut stuck = Vec::with_capacity(remaining.len());
                for managed in &remaining {
                    stuck.push(Box::from(managed.name));
                }
                return Err(Error::Cycle {
                    plugins: stuck.into_boxed_slice(),
                });
            }
            remaining.retain(|managed| !ready.contains(&managed.name));
            order.extend(ready);
        }
        Ok(order)
    }

    /// Check `config` against the plugin's declared spec, coercing it where the spec allows.
    ///
    /// Without the `config-tanzim` feature there is no validator to check against, so the
    /// configuration is passed through as the plugin declared it should be shaped — the plugin
    /// still gets to reject it from `start`.
    #[cfg(feature = "config-tanzim")]
    fn validate(&self, index: usize, config: Value) -> Result<Value> {
        let managed = &self.plugins[index];
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
        let managed = &self.plugins[index];
        for dependency in &managed.info.dependencies {
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
