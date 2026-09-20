use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// The result of a plugin lifecycle call.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a lifecycle call failed.
#[derive(Debug)]
pub enum Error {
    /// The plugin does not implement this operation. A host may fall back to something else — an
    /// unsupported `reload` becomes stop-then-start.
    Unsupported {
        /// Which operation, as a static name: `"reload"`.
        operation: &'static str,
    },
    /// The configuration handed to `start` or `reload` was not usable.
    Config {
        /// The dotted path of the offending value, empty for the config as a whole.
        path: Box<str>,
        /// What was wrong with it.
        message: Box<str>,
    },
    /// The plugin failed for a reason of its own.
    Plugin {
        /// What went wrong.
        source: Box<dyn StdError + Send + Sync>,
    },
    /// Something reached through the context failed: a registration, a dispatch, an export or
    /// a call into another plugin.
    Hook {
        /// What the tables reported.
        source: Box<crate::Error>,
    },
}

impl Error {
    /// Wrap an arbitrary error as a plugin failure.
    pub fn plugin(source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        Self::Plugin {
            source: source.into(),
        }
    }

    /// Report a bad configuration value at `path`.
    pub fn config(path: impl Into<Box<str>>, message: impl Into<Box<str>>) -> Self {
        Self::Config {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Unsupported { operation } => {
                write!(formatter, "this plugin does not support `{operation}`")
            }
            Self::Config { path, message } => {
                if path.is_empty() {
                    write!(formatter, "invalid configuration: {message}")
                } else {
                    write!(formatter, "invalid configuration at `{path}`: {message}")
                }
            }
            Self::Plugin { source } => write!(formatter, "{source}"),
            Self::Hook { source } => write!(formatter, "{source}"),
        }
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Plugin { source } => Some(source.as_ref()),
            Self::Hook { source } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<crate::Error> for Error {
    fn from(source: crate::Error) -> Self {
        Self::Hook {
            source: Box::new(source),
        }
    }
}
