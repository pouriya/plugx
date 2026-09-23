//! The `file://` loader: the two shapes a source can have, and the four ways it can fail.

// Unix-only: proving `Denied` means dropping a file's mode, which is what `PermissionsExt` is.
#![cfg(all(unix, feature = "load-file"))]

use plugx::plugin::load::file::File;
use plugx::plugin::load::{Content, Error, Loader};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A directory of this test's own, removed when the guard drops.
struct Sandbox {
    path: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("plugx-load-file-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create the sandbox");
        Self { path }
    }

    fn source(&self, name: &str) -> String {
        match name {
            "" => format!("file://{}", self.path.display()),
            _ => format!("file://{}", self.path.join(name).display()),
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Best effort: a leftover under `/tmp` is not worth failing a passing test over.
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).expect("write the fixture");
}

#[test]
fn one_regular_file_is_one_artifact() {
    let sandbox = Sandbox::new("one");
    write(&sandbox.path.join("libauth.so"), "not really a library");

    let artifact_list = File::new()
        .load(&sandbox.source("libauth.so"))
        .expect("load one file");

    assert_eq!(artifact_list.len(), 1);
    assert_eq!(artifact_list[0].name, "libauth.so");
    assert_eq!(artifact_list[0].source, sandbox.source("libauth.so"));
    match &artifact_list[0].content {
        Content::Path(path) => assert!(path.ends_with("libauth.so")),
        other => panic!("expected a path, got {other:?}"),
    }
}

#[test]
fn a_directory_is_every_file_in_it_sorted() {
    let sandbox = Sandbox::new("dir");
    write(&sandbox.path.join("libb.so"), "b");
    write(&sandbox.path.join("liba.so"), "a");
    write(&sandbox.path.join("README"), "not a plugin, still reported");
    fs::create_dir(sandbox.path.join("nested")).expect("create a subdirectory");

    let artifact_list = File::new()
        .load(&sandbox.source(""))
        .expect("load the directory");

    let name_list: Vec<&str> = artifact_list
        .iter()
        .map(|artifact| artifact.name.as_str())
        .collect();
    assert_eq!(name_list, ["README", "liba.so", "libb.so"]);
    assert_eq!(artifact_list[1].source, sandbox.source("liba.so"));
}

#[test]
fn an_empty_directory_is_not_an_error() {
    let sandbox = Sandbox::new("empty");
    let artifact_list = File::new()
        .load(&sandbox.source(""))
        .expect("load an empty directory");
    assert!(artifact_list.is_empty());
}

#[test]
fn a_missing_path_is_not_found() {
    let sandbox = Sandbox::new("missing");
    match File::new().load(&sandbox.source("nothing-here.so")) {
        Err(Error::NotFound { source }) => {
            assert_eq!(&*source, sandbox.source("nothing-here.so"));
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn an_unreadable_file_is_denied() {
    let sandbox = Sandbox::new("denied");
    let path = sandbox.path.join("libsecret.so");
    write(&path, "unreadable");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("drop the mode");

    let outcome = File::new().load(&sandbox.source("libsecret.so"));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("restore the mode");

    match outcome {
        Err(Error::Denied { source }) => assert_eq!(&*source, sandbox.source("libsecret.so")),
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[test]
fn an_unreadable_directory_is_denied() {
    let sandbox = Sandbox::new("denied-dir");
    let path = sandbox.path.join("locked");
    fs::create_dir(&path).expect("create the directory");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("drop the mode");

    let outcome = File::new().load(&format!("file://{}", path.display()));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("restore the mode");

    match outcome {
        Err(Error::Denied { source }) => assert!(source.ends_with("locked")),
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[test]
fn something_that_is_neither_is_unusable() {
    let sandbox = Sandbox::new("fifo");
    let path = sandbox.path.join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !made {
        // No `mkfifo` on this machine: there is nothing to assert about, and inventing a device
        // node needs privileges a test does not have.
        return;
    }

    match File::new().load(&format!("file://{}", path.display())) {
        Err(Error::Unusable { source, reason }) => {
            assert!(source.ends_with("pipe"));
            assert_eq!(reason, "not a regular file or a directory");
        }
        other => panic!("expected Unusable, got {other:?}"),
    }
}

#[test]
fn a_source_without_a_scheme_is_a_path() {
    let sandbox = Sandbox::new("bare");
    write(&sandbox.path.join("libauth.so"), "not really a library");
    let bare = sandbox.path.join("libauth.so").display().to_string();

    let artifact_list = File::new().load(&bare).expect("load a bare path");

    assert_eq!(artifact_list.len(), 1);
    assert_eq!(artifact_list[0].name, "libauth.so");
    // The source is kept exactly as it was given, scheme or no scheme: it is what errors and logs
    // quote back, so it has to be the thing the caller typed.
    assert_eq!(artifact_list[0].source, bare);
    assert_eq!(artifact_list[0].content, Content::Path(bare));
}

#[test]
fn a_relative_path_is_a_path_too() {
    // Cargo runs an integration test with the package root as the working directory, so `target`
    // is a relative path that exists. `./` in front of it is what a person types.
    let directory = PathBuf::from("target").join(format!("plugx-relative-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("create the directory");
    write(&directory.join("libauth.so"), "not really a library");

    let dotted = format!("./{}/libauth.so", directory.display());
    let outcome = File::new().load(&dotted);
    let _ = fs::remove_dir_all(&directory);

    let artifact_list = outcome.expect("load a dotted relative path");
    assert_eq!(artifact_list.len(), 1);
    assert_eq!(artifact_list[0].name, "libauth.so");
    assert_eq!(artifact_list[0].source, dotted);
}

#[test]
fn a_missing_bare_path_is_still_not_found() {
    match File::new().load("no-such-directory/libauth.so") {
        Err(Error::NotFound { source }) => assert_eq!(&*source, "no-such-directory/libauth.so"),
        other => panic!("expected NotFound, got {other:?}"),
    }
}
