//! The C ABI, exercised from the other side.
//!
//! Compiles `examples/c_echo_plugin.c` against `include/plugx.h` with the system C compiler, loads
//! the result, and drives it through the ordinary host API. Nothing here knows the plugin is not
//! Rust — which is the point of the test.

#![cfg(feature = "load-cdylib")]

use plugx::{Context, Error, Host, Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build the C plugin, or `None` if this machine has no C compiler.
fn compile() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("examples/c_echo_plugin.c");
    let include = root.join("include");
    let output = Path::new(env!("CARGO_TARGET_TMPDIR")).join("libc_echo_plugin.so");

    let compiler = match std::env::var("CC") {
        Ok(compiler) => compiler,
        Err(_) => "cc".to_string(),
    };
    let result = Command::new(compiler)
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-shared")
        .arg("-fPIC")
        .arg("-I")
        .arg(&include)
        .arg("-o")
        .arg(&output)
        .arg(&source)
        .output();
    let result = match result {
        Ok(result) => result,
        Err(_) => return None,
    };
    if !result.status.success() {
        panic!(
            "the C plugin did not compile:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Some(output)
}

/// One test, because a process has one host: load the C plugin and walk everything it can do.
#[test]
fn a_plugin_written_in_c() {
    let library = match compile() {
        Some(library) => library,
        None => {
            eprintln!("no C compiler; skipping");
            return;
        }
    };

    let mut host = Host::new().expect("one host per process");
    host.export("stamp", |own: &Context, _args: Value| {
        Ok(Value::Str(format!("stamped by {}", own.name())))
    })
    .expect("host export");

    // Named after its file, like any other plugin.
    host.load(&library).expect("load the C plugin");
    let info = host
        .info("c_echo_plugin")
        .expect("the C plugin reported info");
    assert_eq!(info.description, "Shouts strings, from C");

    // Unstarted is its own answer, not "no such function".
    match host
        .context()
        .plugin_call("c_echo_plugin::shout", Value::Str("hi".into()))
    {
        Err(Error::NotStarted { .. }) => {}
        other => panic!("expected NotStarted, got {other:?}"),
    }

    host.start_all(&Map::new()).expect("start");

    // The callback it registered in C runs on a dispatch fired from Rust, and calls the
    // application's own function from inside that dispatch.
    let mut request = Value::map();
    if let Some(map) = request.as_map_mut() {
        map.insert("path", Value::Str("/health".to_string()));
    }
    plugx::run("request.headers", &mut request).expect("dispatch");
    assert_eq!(
        request.get_path("seen-by-c").and_then(Value::as_str),
        Some("c_echo_plugin")
    );
    assert_eq!(
        request.get_path("c-stamp").and_then(Value::as_str),
        Some("stamped by host")
    );

    // The function it exported in C, called by name.
    let shouted = host
        .context()
        .plugin_call("c_echo_plugin::shout", Value::Str("hello".into()))
        .expect("shout");
    assert_eq!(shouted.as_str(), Some("HELLO"));

    // Its own failure crosses as a free-form value.
    match host
        .context()
        .plugin_call("c_echo_plugin::shout", Value::Int(7))
    {
        Err(Error::Failed { plugin, error }) => {
            assert_eq!(&*plugin, "c_echo_plugin");
            assert_eq!(error.as_str(), Some("shout wants a string"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    // A missing function is distinguishable from a missing plugin.
    match host
        .context()
        .plugin_call("c_echo_plugin::whisper", Value::map())
    {
        Err(Error::NoSuchFunction { .. }) => {}
        other => panic!("expected NoSuchFunction, got {other:?}"),
    }

    // Stopping drains the callback and the function together.
    host.stop("c_echo_plugin").expect("stop");
    let mut after = Value::map();
    plugx::run("request.headers", &mut after).expect("dispatch after stop");
    assert!(after.get_path("seen-by-c").is_none());
    match host
        .context()
        .plugin_call("c_echo_plugin::shout", Value::Str("hi".into()))
    {
        Err(Error::NotStarted { .. }) => {}
        other => panic!("expected NotStarted, got {other:?}"),
    }
}
