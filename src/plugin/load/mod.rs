//! # Loaders: where a plugin comes from
//!
//! A [`Loader`] is a **transport**. It claims a URL scheme, fetches whatever the source names, and
//! hands back [`Artifact`]s. It does not know or care what is inside them — that is a
//! [`Runtime`](crate::plugin::runtime::Runtime)'s job, and the two axes multiply rather than pair:
//!
//! ```text
//!   "file://plugins"  ─ Loader ─→  Artifact { name, source, content }  ─ Runtime ─→  Plugin
//!   "https://cdn/…"                                                                  Plugin
//! ```
//!
//! A source that names no scheme is a path, and a path is [`file`](file::File) — see
//! [`split_scheme`]. `plugins`, `./build/libauth.so` and `/opt/plugins` are all sources.
//!
//! Fetching a WebAssembly module over HTTP is `load-http` plus `runtime-wasm`, with nothing
//! written for that combination in particular. Adding a transport gives every format a new place
//! to come from.
//!
//! # A loader does not parse names
//!
//! It reports the name exactly as it found it — `libauth.so`, `redact.wasm` — because the only
//! thing it knows is where the bytes were, and a transport that tried to guess would have to know
//! every format's conventions. The runtime parses it: the extension is how a runtime decides the
//! artifact is its business at all.

/// [`Error`] and this module's [`Result`] alias.
pub mod error;

/// The [`File`](file::File) loader: plugins fetched off this machine's filesystem.
#[cfg(feature = "load-file")]
pub mod file;

pub use error::{Error, Result};

/// Split `scheme://rest` — and if there is no `://`, call the whole thing a `file` path.
///
/// A plugin source is ordinarily `scheme://rest`, but a bare path is what anyone types first, so
/// `plugins`, `./build/libauth.so`, `/opt/plugins` and `C:\plugins` are all sources in their own
/// right and mean exactly what `file://` in front of them would mean. There is no such thing as a
/// source with no scheme; there is only one that did not say it.
///
/// ```
/// use plugx::plugin::load::split_scheme;
///
/// assert_eq!(split_scheme("https://cdn/plugins.json"), ("https", "cdn/plugins.json"));
/// assert_eq!(split_scheme("file://C:\\plugins"), ("file", "C:\\plugins"));
/// assert_eq!(split_scheme("./x/y/z"), ("file", "./x/y/z"));
/// assert_eq!(split_scheme("libauth.so"), ("file", "libauth.so"));
/// ```
///
/// Used twice: by a [`Host`](crate::Host) to pick the loader, and by that loader to get at the
/// part after the scheme.
pub fn split_scheme(source: &str) -> (&str, &str) {
    match source.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        // A Windows path is safe here: `C:\plugins` has no `://`, and `C:/plugins` has no `://`
        // either, so neither is mistaken for a scheme.
        None => ("file", source),
    }
}

/// An artifact's bytes, or where a runtime can find them.
///
/// Two shapes because the formats genuinely differ. A WebAssembly module or a Starlark script can
/// be handed over as bytes and never touch a disk. A shared library cannot: `dlopen` takes a path,
/// and there is no portable way to map one out of memory — Linux needs `memfd_create` and
/// `/proc/self/fd`, and macOS and Windows have no supported route at all.
///
/// A loader reports whichever it has. The filesystem has paths, so [`File`](file::File) reports
/// [`Path`](Content::Path); an HTTP loader has bytes and reports [`Bytes`](Content::Bytes), which
/// a cdylib runtime refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// A path on this machine's filesystem.
    ///
    /// A `String`, not a `Path`: it crosses to runtimes that may never touch a filesystem, and it
    /// is carried next to the source string it came from.
    Path(String),
    /// The bytes themselves, already in hand.
    Bytes(Vec<u8>),
}

/// One thing a [`Loader`] fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// The name as the loader found it, unparsed: `libauth.so`, `redact.wasm`.
    pub name: String,
    /// Where it came from, in full: `file://plugins/libauth.so`.
    ///
    /// For logs and errors, and the only thing identifying an artifact before a runtime has had a
    /// look at it.
    pub source: String,
    /// The artifact itself.
    pub content: Content,
}

/// A transport: somewhere plugins can be fetched from.
pub trait Loader: Send + Sync {
    /// A short name for this loader, as it appears in logs: `"file"`, `"http"`.
    fn name(&self) -> &str;

    /// The URL schemes this loader claims, without the `://`: `["http", "https"]`.
    ///
    /// A [`Host`](crate::Host) hands a source to the first loader claiming its scheme, and refuses
    /// a source no loader claims. A source that names no scheme is a `file` one.
    fn schema_list(&self) -> &[&str];

    /// Fetch everything `source` names.
    ///
    /// A `Vec`, because a source may name a container of artifacts — a directory, an index — as
    /// easily as a single one. An empty `Vec` is not an error: a place plugins can be is allowed
    /// to have none in it.
    fn load(&self, source: &str) -> Result<Vec<Artifact>>;
}
