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

    // 3. A source no loader claims is refused up front, before anything is fetched.
    match host.add_plugin_source("ftp://plugins") {
        Err(plugx::host::Error::Load { .. }) => println!("unknown scheme refused"),
        other => panic!("expected an unknown scheme to be refused, got {other:?}"),
    }

    //    A path that is not there is its own error, and it names the source that was not there.
    host.add_plugin_source("file:///nonexistent/plugins")
        .expect("add a missing directory");
    match host.load_all() {
        Err(plugx::host::Error::Load { source }) => match *source {
            plugx::plugin::load::Error::NotFound { source } => {
                println!("missing source reported: {source}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        },
        other => panic!("expected a missing source to be refused, got {other:?}"),
    }

    // 4. Fetch a directory. Every file in it goes to the runtime that claims its extension, and
    //    each plugin is named after its file, so a repeat is refused.
    host.add_plugin_source(&format!("file://{}", directory.display()))
        .expect("add the plugin directory");
    host.load_all().expect("load all");
    // A source with no `://` is a plain path, which is the same thing said shorter.
    host.add_plugin_source(&echo.display().to_string())
        .expect("add one plugin twice");
    match host.load_all() {
        Err(plugx::host::Error::Duplicate { .. }) => println!("duplicate name refused"),
        other => panic!("expected a duplicate name to be refused, got {other:?}"),
    }
    let _ = &caller;

    // 5. Calling a loaded-but-unstarted plugin is its own error.
    match host
        .context()
        .plugin_call("echo_plugin::reverse", Value::Str("x".into()))
    {
        Err(Error::NotStarted { .. }) => println!("unstarted plugin reported"),
        other => panic!("expected NotStarted, got {other:?}"),
    }

    host.start_all(&plugx::Map::new()).expect("start all");
    for (name, state) in host.plugin_list() {
        println!("{name} is {}", state.label());
    }

    // 6. A hook fired by the application, through the free function — no context threaded
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

    // 7. Direct calls, and the three ways one can miss.
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

    // 8. A plugin's own error crosses as a free-form value.
    match host
        .context()
        .plugin_call("echo_plugin::reverse", Value::Int(7))
    {
        Err(Error::Failed { plugin, error }) => println!("{plugin} failed with {error:?}"),
        other => panic!("expected Failed, got {other:?}"),
    }

    // 9. Stop drains hooks and functions together.
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

    // 10. With the host gone the slot is free again, and the free function has nothing to fire
    //    into.
    drop(host);
    match plugx::run("request.headers", &mut Value::map()) {
        Err(Error::NoHost) => println!("no host, no dispatch"),
        other => panic!("expected NoHost, got {other:?}"),
    }
    Host::new().expect("the slot is free again");
    println!("done");
}
