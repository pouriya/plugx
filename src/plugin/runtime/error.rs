use crate::abi::AbiVersion;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// The result of building an artifact.
pub type Result<T> = std::result::Result<T, Error>;

/// Why an artifact could not be turned into plugins.
///
/// Everything here is a format's problem. *Getting* the artifact is a
/// [`loader`](crate::plugin::load)'s, and fails with its error instead.
#[derive(Debug)]
pub enum Error {
    /// No registered runtime claims the artifact's extension.
    Unsupported {
        /// Where the artifact came from.
        source: Box<str>,
    },
    /// The artifact's name is not one a plugin can be called.
    Unnamed {
        /// Where the artifact came from.
        source: Box<str>,
    },
    /// The runtime cannot take content in this shape.
    Unusable {
        /// Where the artifact came from.
        source: Box<str>,
        /// Why not.
        reason: &'static str,
    },
    /// The artifact could not be opened.
    Open {
        /// Where the artifact came from.
        source: Box<str>,
        /// What the platform reported.
        message: Box<str>,
    },
    /// The artifact opened but does not export a symbol every plugin must.
    MissingSymbol {
        /// The symbol that was not there.
        symbol: &'static str,
        /// Where the artifact came from.
        source: Box<str>,
    },
    /// The plugin was built against an ABI this host cannot speak.
    IncompatibleAbi {
        /// What the plugin reported.
        plugin: AbiVersion,
        /// What this host speaks.
        host: AbiVersion,
    },
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Unsupported { source } => write!(formatter, "no runtime recognises `{source}`"),
            Self::Unnamed { source } => {
                write!(formatter, "`{source}` has no name to call a plugin by")
            }
            Self::Unusable { source, reason } => {
                write!(formatter, "`{source}` cannot be run: {reason}")
            }
            Self::Open { source, message } => {
                write!(formatter, "could not open `{source}`: {message}")
            }
            Self::MissingSymbol { symbol, source } => write!(
                formatter,
                "`{source}` does not export `{symbol}` — is it a plugx plugin?"
            ),
            Self::IncompatibleAbi { plugin, host } => write!(
                formatter,
                "plugin speaks plugx ABI {plugin}, this host speaks {host}"
            ),
        }
    }
}

impl StdError for Error {}
