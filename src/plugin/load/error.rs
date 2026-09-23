use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::io::{Error as IoError, ErrorKind};

/// The result of fetching a source.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a plugin source could not be fetched.
///
/// Everything here is a transport's problem. What is *inside* an artifact is a
/// [`runtime`](crate::plugin::runtime)'s, and fails with its error instead.
///
/// **Every variant names the source it is about**, and names the exact one: a directory that
/// failed halfway through reports the entry that failed, not the directory it was asked for. A
/// host loads from several sources at once, so an error that only said "permission denied" would
/// leave the caller to guess which of them it came from.
///
/// [`NotFound`](Self::NotFound), [`Denied`](Self::Denied) and [`Timeout`](Self::Timeout) are the
/// failures worth handling differently — a missing directory is usually a deployment mistake, a
/// denied one a packaging mistake, a timeout worth a retry — so they are their own variants rather
/// than a message a caller would have to match on as text. [`Fetch`](Self::Fetch) is everything
/// rarer than those.
#[derive(Debug)]
pub enum Error {
    /// No registered loader claims the scheme this source names.
    ///
    /// A source with no `://` names `file`, so this is only ever a scheme that was spelled out.
    UnknownScheme {
        /// What was offered.
        source: Box<str>,
    },
    /// Nothing is there: no such file, no such directory, no such object.
    NotFound {
        /// What was not there.
        source: Box<str>,
    },
    /// It is there, and this process may not read it.
    Denied {
        /// What could not be read.
        source: Box<str>,
    },
    /// The transport gave up waiting.
    ///
    /// A remote loader's ordinary failure. The local filesystem can raise it too — a hung network
    /// mount — which is why it is not an HTTP-only concern.
    Timeout {
        /// What was being fetched when the clock ran out.
        source: Box<str>,
    },
    /// The source names something that exists and is not an artifact.
    ///
    /// A transport knows the shapes its own medium has: a filesystem path can be a socket, a fifo
    /// or a device node as easily as a file, and none of those is something a runtime can be
    /// handed.
    Unusable {
        /// What was named.
        source: Box<str>,
        /// What it is instead, in a few words.
        reason: &'static str,
    },
    /// The loader could not fetch what the source names, for any other reason.
    Fetch {
        /// What was being fetched.
        source: Box<str>,
        /// What the transport reported.
        message: Box<str>,
    },
}

impl Error {
    /// Classify an [`io::Error`](IoError) against the source it happened to.
    ///
    /// The three kinds that get their own variant are recognised; anything else becomes a
    /// [`Fetch`](Self::Fetch) carrying the message. Any loader touching a filesystem, a socket or a
    /// pipe should build its errors through here, so that callers get the same three cases out of
    /// every transport.
    pub fn io(source: &str, error: &IoError) -> Self {
        match error.kind() {
            ErrorKind::NotFound => Self::NotFound {
                source: source.into(),
            },
            ErrorKind::PermissionDenied => Self::Denied {
                source: source.into(),
            },
            ErrorKind::TimedOut => Self::Timeout {
                source: source.into(),
            },
            _ => Self::Fetch {
                source: source.into(),
                message: error.to_string().into_boxed_str(),
            },
        }
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::UnknownScheme { source } => {
                write!(formatter, "no loader claims the scheme in `{source}`")
            }
            Self::NotFound { source } => write!(formatter, "`{source}` does not exist"),
            Self::Denied { source } => {
                write!(formatter, "not allowed to read `{source}`")
            }
            Self::Timeout { source } => write!(formatter, "timed out fetching `{source}`"),
            Self::Unusable { source, reason } => {
                write!(formatter, "`{source}` is {reason}")
            }
            Self::Fetch { source, message } => {
                write!(formatter, "could not fetch `{source}`: {message}")
            }
        }
    }
}

impl StdError for Error {}
