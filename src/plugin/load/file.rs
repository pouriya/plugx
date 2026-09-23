//! Plugins fetched off this machine's filesystem.

use crate::plugin::load::{Artifact, Content, Error, Loader, Result, split_scheme};
use cfg_if::cfg_if;

/// Fetches plugins from a path: `file://plugins`, or `file://plugins/libauth.so`.
///
/// A directory yields every file directly inside it, sorted by name, each with its own filename
/// appended to the source — so `file://plugins` becomes `file://plugins/libauth.so`. Anything else
/// yields exactly one artifact. Subdirectories are not descended into: a plugin directory is a
/// list, not a tree.
///
/// Nothing here decides whether a file is a plugin. Every file is reported, and the runtimes sort
/// it out by extension — a directory holding a README and a config file is normal, and both are
/// skipped later, in silence.
///
/// # Failures
///
/// A missing path is [`NotFound`](Error::NotFound), an unreadable one [`Denied`](Error::Denied),
/// and each names the exact path it is about: when a directory listing fails on one entry, the
/// error names that entry's source, not the directory that was asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct File;

impl File {
    /// A loader for the local filesystem.
    pub const fn new() -> Self {
        Self
    }
}

impl Loader for File {
    fn name(&self) -> &str {
        "file"
    }

    fn schema_list(&self) -> &[&str] {
        &["file"]
    }

    fn load(&self, source: &str) -> Result<Vec<Artifact>> {
        // A bare path is a `file` source that did not say so, and arrives here unchanged.
        let (_, path) = split_scheme(source);

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                let _span = tracing::debug_span!("plugin.fetch", loader = "file").entered();
                tracing::debug!(msg = "Fetching plugin source", path = path);
            } else if #[cfg(feature = "logging")] {
                log::debug!("msg=\"Fetching plugin source\" loader=file path={path}");
            }
        }

        // One `metadata` call sorts the source into its shape: a single file to report as it
        // stands, or a directory to list. It follows symlinks, so a link to either is that one.
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => return Err(Error::io(source, &error)),
        };

        if metadata.is_file() {
            // `metadata` proves the path is there; only opening it proves this process may read
            // it. Permissions are the transport's business, so they are answered here, as
            // `Denied` against this source — otherwise the first thing to notice is a runtime,
            // which reports it as a load failure with the real reason buried in a message. This
            // is a check and not a guarantee: the mode can change before the runtime opens it.
            match std::fs::File::open(path) {
                Ok(_) => {}
                Err(error) => return Err(Error::io(source, &error)),
            }

            let name = match path.rsplit_once(['/', '\\']) {
                Some((_, name)) => name,
                None => path,
            };

            cfg_if! {
                if #[cfg(feature = "tracing")] {
                    tracing::trace!(msg = "Fetched one plugin file", path = path);
                } else if #[cfg(feature = "logging")] {
                    log::trace!("msg=\"Fetched one plugin file\" loader=file path={path}");
                }
            }

            return Ok(vec![Artifact {
                name: name.to_string(),
                source: source.to_string(),
                content: Content::Path(path.to_string()),
            }]);
        }

        // A third shape, and the reason the two above are tested for rather than assumed: a socket,
        // a fifo or a device node is neither, and handing one to a runtime turns a wrong path into
        // a confusing failure much further along — a fifo `dlopen`s as a corrupt library, and a
        // runtime that reads its content blocks forever.
        if !metadata.is_dir() {
            return Err(Error::Unusable {
                source: source.into(),
                reason: "not a regular file or a directory",
            });
        }

        let trimmed = source.trim_end_matches('/');
        let base = path.trim_end_matches(['/', '\\']);
        let entry_list = match std::fs::read_dir(path) {
            Ok(entry_list) => entry_list,
            Err(error) => return Err(Error::io(source, &error)),
        };

        // Collected and sorted, not streamed, so that a directory loads in the same order on every
        // platform — a plugin's priority is its own, but ties break by load order.
        let mut name_list = Vec::new();
        for entry in entry_list {
            let entry = match entry {
                Ok(entry) => entry,
                // Nothing identifies which entry this was: the iterator failed before producing
                // one, so the directory is the most precise source there is.
                Err(error) => return Err(Error::io(source, &error)),
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => continue,
                Ok(_) => {}
                Err(error) => return Err(Error::io(&format!("{trimmed}/{name}"), &error)),
            }
            name_list.push(name);
        }
        name_list.sort();

        let mut artifact_list = Vec::with_capacity(name_list.len());
        for name in name_list {
            artifact_list.push(Artifact {
                source: format!("{trimmed}/{name}"),
                content: Content::Path(format!("{base}/{name}")),
                name,
            });
        }

        cfg_if! {
            if #[cfg(feature = "tracing")] {
                tracing::trace!(msg = "Fetched plugin directory", count = artifact_list.len());
            } else if #[cfg(feature = "logging")] {
                log::trace!(
                    "msg=\"Fetched plugin directory\" loader=file path={path} count={}",
                    artifact_list.len()
                );
            }
        }

        Ok(artifact_list)
    }
}
