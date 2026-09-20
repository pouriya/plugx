use crate::plugin::Version;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// The result of a host operation.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a host operation failed.
#[derive(Debug)]
pub enum Error {
    /// No loader could turn the artifact into a plugin.
    Load {
        /// What the loader reported.
        source: Box<crate::load::Error>,
    },
    /// A lifecycle call into the plugin failed.
    Plugin {
        /// The plugin the host was talking to.
        plugin: Box<str>,
        /// What it reported.
        source: Box<crate::plugin::Error>,
    },
    /// The tables refused to drain the plugin in time.
    ///
    /// Its callbacks and functions are out of the tables, so the plugin can receive nothing new,
    /// but at least one dispatch or call was still running. The plugin was **not** stopped; retry.
    Drain {
        /// The plugin being stopped.
        plugin: Box<str>,
        /// What the tables reported.
        source: Box<crate::Error>,
    },
    /// A plugin of that name is already loaded. The name is the identity, so it must be unique.
    Duplicate {
        /// The name that was already taken.
        plugin: Box<str>,
    },
    /// No plugin is registered under that name or id.
    Unknown {
        /// What was asked for.
        plugin: Box<str>,
    },
    /// The operation does not apply in the plugin's current state.
    WrongState {
        /// The plugin.
        plugin: Box<str>,
        /// What state it is in.
        state: &'static str,
        /// What state the operation needs.
        expected: &'static str,
    },
    /// A declared dependency is not loaded, or is loaded at an unacceptable version.
    Dependency {
        /// The plugin that declared it.
        plugin: Box<str>,
        /// What it needs.
        needs: Box<str>,
        /// The version present, if the plugin is loaded at all.
        found: Option<Version>,
    },
    /// Dependencies form a cycle, so there is no order that starts everything.
    Cycle {
        /// The plugins caught in it.
        plugins: Box<[Box<str>]>,
    },
    /// Another host is already running in this process.
    ///
    /// There is one slot for a program's tables, and one host fills it — that is what lets a
    /// library fire hooks with [`plugx::run`](crate::run) without being handed anything. Drop the
    /// first host to build a second.
    HostExists,
    /// A path has no filename to take a plugin's name from.
    Unnamed {
        /// The path that was offered.
        path: Box<str>,
    },
    /// A directory offered for scanning could not be read.
    Directory {
        /// The directory.
        path: Box<str>,
        /// What the filesystem reported.
        message: Box<str>,
    },
    /// The configuration did not match the plugin's declared spec.
    Config {
        /// The plugin whose config it is.
        plugin: Box<str>,
        /// What was wrong.
        message: Box<str>,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Load { source } => write!(formatter, "{source}"),
            Self::Plugin { plugin, source } => write!(formatter, "plugin `{plugin}`: {source}"),
            Self::Drain { plugin, source } => {
                write!(formatter, "stopping plugin `{plugin}`: {source}")
            }
            Self::Duplicate { plugin } => {
                write!(formatter, "a plugin called `{plugin}` is already loaded")
            }
            Self::Unknown { plugin } => write!(formatter, "no plugin `{plugin}` is loaded"),
            Self::WrongState {
                plugin,
                state,
                expected,
            } => write!(
                formatter,
                "plugin `{plugin}` is {state}, and this needs it to be {expected}"
            ),
            Self::Dependency {
                plugin,
                needs,
                found,
            } => match found {
                Some(version) => write!(
                    formatter,
                    "plugin `{plugin}` needs {needs}, but version {version} is loaded"
                ),
                None => write!(
                    formatter,
                    "plugin `{plugin}` needs {needs}, which is not loaded"
                ),
            },
            Self::Cycle { plugins } => {
                formatter.write_str("dependency cycle between:")?;
                for plugin in plugins {
                    write!(formatter, " `{plugin}`")?;
                }
                Ok(())
            }
            Self::HostExists => {
                formatter.write_str("another plugx host is already running in this process")
            }
            Self::Unnamed { path } => {
                write!(formatter, "`{path}` has no filename to name a plugin after")
            }
            Self::Directory { path, message } => {
                write!(formatter, "could not read `{path}`: {message}")
            }
            Self::Config { plugin, message } => {
                write!(formatter, "configuration for `{plugin}`: {message}")
            }
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Load { source } => Some(source.as_ref()),
            Self::Plugin { source, .. } => Some(source.as_ref()),
            Self::Drain { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<crate::load::Error> for Error {
    fn from(source: crate::load::Error) -> Self {
        Self::Load {
            source: Box::new(source),
        }
    }
}
