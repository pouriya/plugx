use crate::abi::AbiVersion;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::path::PathBuf;

/// The result of a load.
pub type Result<T> = std::result::Result<T, Error>;

/// Why an artifact could not be turned into a plugin.
#[derive(Debug)]
pub enum Error {
    /// No registered loader recognised the artifact.
    Unsupported {
        /// What was being loaded.
        path: PathBuf,
    },
    /// The artifact could not be opened.
    Open {
        /// What was being loaded.
        path: PathBuf,
        /// What the platform reported.
        message: Box<str>,
    },
    /// The artifact opened but does not export a symbol every plugin must.
    MissingSymbol {
        /// The symbol that was not there.
        symbol: &'static str,
        /// What was being loaded.
        path: PathBuf,
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
            Self::Unsupported { path } => {
                write!(formatter, "no loader recognises `{}`", path.display())
            }
            Self::Open { path, message } => {
                write!(formatter, "could not open `{}`: {message}", path.display())
            }
            Self::MissingSymbol { symbol, path } => write!(
                formatter,
                "`{}` does not export `{symbol}` — is it a plugx plugin?",
                path.display()
            ),
            Self::IncompatibleAbi { plugin, host } => write!(
                formatter,
                "plugin speaks plugx ABI {plugin}, this host speaks {host}"
            ),
        }
    }
}

impl StdError for Error {}
