use crate::value::Kind;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter, Result as FmtResult};

/// What went wrong when reading a [`Value`](crate::Value).
///
/// Kept field-light on purpose: an [`Error`] is returned by value from accessors, so it has to stay
/// under the `clippy::result_large_err` threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// The value was of the wrong kind for the requested accessor.
    TypeMismatch {
        /// The kind the caller asked for.
        expected: Kind,
        /// The kind actually present.
        found: Kind,
    },
    /// A map did not contain the requested key.
    MissingKey,
    /// A list index was past the end.
    IndexOutOfRange {
        /// The index that was asked for.
        index: u32,
        /// The length of the list.
        len: u32,
    },
}

/// A failed read of a [`Value`](crate::Value), carrying the dotted path that was being walked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    path: Box<str>,
}

impl Error {
    /// Build an error for `path`.
    pub fn new(kind: ErrorKind, path: impl Into<Box<str>>) -> Self {
        Self {
            kind,
            path: path.into(),
        }
    }

    /// What went wrong.
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// The dotted path being walked when it went wrong. Empty for a read of the root value.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match &self.kind {
            ErrorKind::TypeMismatch { expected, found } => {
                write!(formatter, "expected {expected}, found {found}")?;
            }
            ErrorKind::MissingKey => {
                write!(formatter, "no such key")?;
            }
            ErrorKind::IndexOutOfRange { index, len } => {
                write!(
                    formatter,
                    "index {index} is past the end of a {len}-item list"
                )?;
            }
        }
        if !self.path.is_empty() {
            write!(formatter, " at `{}`", self.path)?;
        }
        Ok(())
    }
}

impl StdError for Error {}
