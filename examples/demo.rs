//! Loads the two cdylib examples and walks the whole surface end to end.
//!
//! ```text
//! make examples && ./bin/demo
//! ```
//!
//! Pass the directory holding `libecho_plugin.so` and `libcaller_plugin.so` as the first argument;
//! it defaults to `target/release/examples`. The plugins are named after their files, so they load
//! as `echo_plugin` and `caller_plugin`.

use plugx::{Context, Error, Host, Value};
use std::path::PathBuf;

fn main() {
    let directory = match std::env::args().nth(1) {
        Some(directory) => PathBuf::from(directory),
        None => PathBuf::from("target/release/examples"),
    };
    let extension = match std::env::consts::DLL_EXTENSION {
        "" => "so",
        other => other,
    };
    let echo = directory.join(format!("libecho_plugin.{extension}"));
    let caller = directory.join(format!("libcaller_plugin.{extension}"));

    let mut host = Host::new().expect("one host per process");

    // 1. The application's own function, callable by any plugin.
    host.export("stamp", |own: &Context, _args: Value| {
        Ok(Value::Str(format!("stamped by {}", own.name())))
    })
    .expect("host export");

    // 2. One host per process: the slot is taken until this one is dropped.
    match Host::new() {
        Err(plugx::host::Error::HostExists) => println!("second host refused"),
        other => panic!("expected HostExists, got {}", other.is_ok()),
    }

    // 3. Scan a directory. Each file goes to the loader that claims its extension, and each
    //    plugin is named after its file, so a repeat is refused.
    host.add_dir(&directory);
    host.load_all().expect("load all");
    match host.load(&echo) {
        Err(plugx::host::Error::Duplicate { .. }) => println!("duplicate name refused"),
        other => panic!("expected a duplicate name to be refused, got {other:?}"),
    }
    let _ = &caller;

    // 4. Calling a loaded-but-unstarted plugin is its own error.
    match host
        .context()
        .plugin_call("echo_plugin::reverse", Value::Str("x".into()))
    {
        Err(Error::NotStarted { .. }) => println!("unstarted plugin reported"),
        other => panic!("expected NotStarted, got {other:?}"),
    }

    host.start_all(&plugx::Map::new()).expect("start all");
    for (name, state) in host.plugins() {
        println!("{name} is {}", state.label());
    }

    // 5. A hook fired by the application, through the free function — no context threaded
    //    anywhere. `caller_plugin`'s callback calls `echo_plugin::reverse` and the host's `stamp`
    //    from inside the dispatch.
    let mut request = Value::map();
    if let Some(map) = request.as_map_mut() {
        map.insert("path", Value::Str("/health".to_string()));
    }
    plugx::run("request.headers", &mut request).expect("dispatch");
    println!("after dispatch: {request:?}");
    assert_eq!(
        request.get_path("reversed").and_then(Value::as_str),
        Some("htlaeh/")
    );
    assert_eq!(
        request.get_path("seen-by").and_then(Value::as_str),
        Some("echo_plugin")
    );

    // 6. Direct calls, and the three ways one can miss.
    let pong = host
        .context()
        .plugin_call("caller_plugin::ping", Value::map())
        .expect("ping");
    println!("caller_plugin::ping -> {pong:?}");

    match host
        .context()
        .plugin_call("echo_plugin::missing", Value::map())
    {
        Err(Error::NoSuchFunction { .. }) => println!("missing function reported"),
        other => panic!("expected NoSuchFunction, got {other:?}"),
    }
    match host.context().plugin_call("ghost::f", Value::map()) {
        Err(Error::NoSuchPlugin { .. }) => println!("missing plugin reported"),
        other => panic!("expected NoSuchPlugin, got {other:?}"),
    }
    match host.context().plugin_call("echo-reverse", Value::map()) {
        Err(Error::Malformed { .. }) => println!("malformed target reported"),
        other => panic!("expected Malformed, got {other:?}"),
    }

    // 7. A plugin's own error crosses as a free-form value.
    match host
        .context()
        .plugin_call("echo_plugin::reverse", Value::Int(7))
    {
        Err(Error::Failed { plugin, error }) => println!("{plugin} failed with {error:?}"),
        other => panic!("expected Failed, got {other:?}"),
    }

    // 8. Stop drains hooks and functions together.
    host.stop("echo_plugin").expect("stop echo");
    match host
        .context()
        .plugin_call("echo_plugin::reverse", Value::Str("x".into()))
    {
        Err(Error::NotStarted { .. }) => println!("stopped plugin is no longer callable"),
        other => panic!("expected NotStarted, got {other:?}"),
    }
    let mut request = Value::map();
    plugx::run("request.headers", &mut request)
        .expect_err("caller's callback should now fail to reach echo");

    host.stop_all().expect("stop all");

    // 9. With the host gone the slot is free again, and the free function has nothing to fire
    //    into.
    drop(host);
    match plugx::run("request.headers", &mut Value::map()) {
        Err(Error::NoHost) => println!("no host, no dispatch"),
        other => panic!("expected NoHost, got {other:?}"),
    }
    Host::new().expect("the slot is free again");
    println!("done");
}
