# plugx

**Modular, extensible and observable plugin framework for Rust.**

Named hooks, callbacks with priorities, plugin-to-plugin function calls, and plugins loaded from
shared libraries — with a dependency footprint that matches what you actually are. A library that
just wants extension points pulls in the registry and nothing else. An application that hosts plugins
takes the runtime. A plugin author takes the SDK and never sees the C layer.

## Why

Every extensible program grows the same three problems, and they are usually solved three
incompatible times: *where can behaviour be changed*, *how does foreign code get loaded*, and *how
does any of it get taken back out again without crashing*.

plugx treats those as one design. A hook is a name and a payload. A plugin is four lifecycle
methods. Loading is a trait with one implementation per artifact kind. And unloading — the part
that quietly ruins plugin systems — is answered once, at the framework level, rather than left to
each host to get wrong.

## Two ways in

Code linked into your application fires hooks through one free function:

```rust
plugx::run("request.headers", &mut data)?;
```

It reads this program's registry out of the one slot a `Host` fills when it is built and clears when
it is dropped. That is what lets a library with extension points in it stay a library: it fires
hooks without the application threading anything through.

Code inside a loaded plugin is handed a `Context` on every call instead, and does everything
through it:

```rust
context.run("request.headers", &mut data)?;          // fire a hook
context.on_transform("log.record", 0, redact)?;      // register a callback
context.export("reverse", reverse)?;                 // publish your own function
context.plugin_call("echo::reverse", args)?;         // call another plugin's
context.host_call("now", args)?;                     // call the application's
```

A `Context` is 32 bytes — a name and where its registry is — `Copy` and `'static`, so nothing caches
one for you and a plugin can keep its copy for a background thread. A plugin `.so` links its own
copy of plugx, so the slot inside it is empty and stays empty: `plugx::run` from in there reports
`NoHost` rather than swallowing the call into a private table. One slot also means one live `Host`
per process — a second one is refused until the first is dropped.

## Principles

- **Nearly free where nothing is listening.** A hook with no callbacks costs one relaxed atomic
  load, a mask and a branch. That is what makes it reasonable to put hooks in hot paths, which is
  where extension points are actually wanted.
- **Dispatch holds no lock.** Callbacks run with nothing held. Threads dispatch in parallel, a
  callback may register another or fire a nested hook, and a slow callback blocks nobody.
- **One consistent view per dispatch.** A `run` sees the same set of callbacks from start to
  finish. Stopping a plugin mid-dispatch lets in-flight callbacks complete; only the thread calling
  `stop` waits.
- **Leaking is always safe; unloading is not.** A loaded library is never `dlclose`d. Stopping
  drains callbacks and drops state while the code is still mapped, then the mapping stays for the
  life of the process. Code reload opens a fresh copy at a new path.
- **Nothing Rust-owned crosses the ABI.** Only pointer+length slices the receiver copies, and
  opaque handles freed by their owner. A plugin has its own allocator; the boundary respects that.
- **Pay for what you use.** Every loader, every runtime, the host, the plugin contract and the SDK
  are cargo features, all off by default, so a library author compiles the registry alone.

## The modules

| Module | Feature | Purpose |
|--------|---------|---------|
| `plugx::value` | — | The value carried through hooks |
| `plugx::abi` | — | `AbiVersion`; `abi::cdylib` is the frozen `repr(C)` host/plugin contract |
| `plugx::context` | — | The `Context` a plugin is reached through |
| `plugx::registry` | — | The four tables a host owns, the one slot holding them, and the free `run` |
| `plugx::hook` | — | Declaring hooks, and the two callback traits |
| `plugx::error` | — | One `Error` for everything a context can fail at |
| `plugx::plugin` | `plugin` | The `Plugin` trait, `Info`, `ConfigSpec`, `Dependency` |
| `plugx::plugin::load` | `host` | Where a plugin comes from: the `Loader` contract, `Artifact` |
| `plugx::plugin::load::file` | `load-file` | The `file://` loader, and the default for a bare path |
| `plugx::plugin::runtime` | `host` | What a plugin is run as: the `Runtime` contract |
| `plugx::plugin::runtime::cdylib` | `runtime-cdylib` | The cdylib runtime |
| `plugx::host` | `host` | Dependency resolution, lifecycle (discovery: `load-file`) |
| `plugx::sdk::cdylib` | `compile-cdylib` | Writing plugins in Rust as cdylib |
| `plugx::testing` | `testing` | Test harness for the concurrent parts |

## Using it

**A library exposing extension points** — fire, and let the application decide if anything listens:

```rust
plugx::hook!(pub REQUEST_HEADERS = "request.headers");

fn handle(headers: &mut Value) -> plugx::Result<Flow> {
    plugx::run(&REQUEST_HEADERS, headers)           // a &str works too
}
```

**An application hosting plugins** — it owns the registry and fills the slot:

```rust
let mut host = plugx::Host::new()?;
host.export("now", |_: &Context, _args| Ok(Value::Int(now())))?;
host.add_dir("plugins");                            // libauth.so loads as `auth`
host.load_all()?;
host.start_all(&configs)?;
handle(&mut headers)?;
```

**A plugin in C** — there is no library to link, only [include/plugx.h](include/plugx.h):
[examples/c_echo_plugin.c](examples/c_echo_plugin.c) exports a function, registers a hook callback
and calls the application from inside a dispatch, in 200 lines with no Rust in sight.

**A plugin:**

```rust
use plugx::sdk::prelude::*;

impl Plugin for Redact {
    fn start(&self, context: &Context, _config: &Value) -> Result<()> {
        context.on_transform("log.record", 0, redact)?;
        context.export("redact", redact_value)?;
        Ok(())
    }
    // info, stop …
}

plugx::export_plugin!(Redact);
```

A callback is handed its own context, so from inside a dispatch it can call another plugin, call the
application, fire further hooks, and register or withdraw whatever it likes. Cross-plugin cycles are
not detected — that one is the plugin author's.

## Two kinds of callback

A **`Transform`** receives `&mut Value` and may rewrite the payload. An **`Observe`** receives
`&Value` and may only react. They share one priority-ordered list per hook, so they interleave
however you schedule them, and either can end a dispatch by returning `Flow::Stop`.

## Development

```
make all            # build, clippy, test, check-style
make clippy         # the gate: --all-features --all-targets -D warnings
make examples       # build the two cdylib plugins and the host demo into bin/
./bin/demo bin      # walk the whole surface end to end
cargo test          # includes tests/abi.rs, which compiles and loads a plugin written in C
```

See [AGENTS.md](AGENTS.md) for the conventions this crate is written to.

## License

BSD-3-Clause. See [LICENSE](LICENSE).
