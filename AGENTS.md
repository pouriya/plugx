# plugx

A plugin framework: **hooks** (named extension points) dispatched to **callbacks** registered by
**plugins** — plus **functions** those plugins export to each other and to the application.

Plugins come in on two independent axes, and every cargo feature names one of them: a **loader**
fetches an artifact from somewhere (`load-file`, `load-http`), and a **runtime** runs what was
fetched (`runtime-cdylib`, `runtime-wasm`, `runtime-starlark`, `runtime-js`). They multiply rather
than pair: an HTTP-fetched wasm module is `load-http` + `runtime-wasm`, with nothing written for
that combination specifically. `compile-cdylib` is the third, unrelated axis: building a plugin of
that kind in Rust.

Application code fires hooks with **`plugx::run`**, which reads this program's registry out of one
global slot. Plugin code is handed a **`Context`** — a name and a pointer to the registry — on every
call and reaches the host through that. Those are the only two doors, and the slot is the only
global in the crate.

## Modules

One crate, one module per stage. Everything past the registry is behind a cargo feature, all off by
default, so a library author that only fires hooks compiles the always-on modules and nothing else.

| Module | Feature | Purpose |
|--------|---------|---------|
| `value` | — | The dynamic value carried through hooks and calls (`Value`, `Map`, `Kind`, `Error`) |
| `abi` | — | `AbiVersion`, plus one submodule per plugin kind. `abi::cdylib` is the frozen `repr(C)` contract (vtables, entry symbols) |
| `error` | — | One `Error` for everything a `Context` can fail at |
| `context` | — | `Context`, `HostAccess` and `HostOps` — the whole plugin-author surface, naming no plugin kind |
| `registry` | — | `Registry`: the four tables one host owns, the one slot holding this program's, the free `run`, plus `drain` / `Drained` (`ApiFn`, `State`) |
| `hook` | — | Declaring hooks and the two callback traits (`Hook`, `Transform`, `Observe`, `Flow`) |
| `plugin` | `plugin` | The plugin contract (`Plugin`, `Info`, `ConfigSpec`, `Dependency`), plus one submodule per kind |
| `plugin::load` | `host` | Axis one, transport: the `Loader` contract, `Artifact`, `Content` |
| `plugin::load::file` | `load-file` | The `file://` loader, and what a source with no scheme means: a path, or every file in a directory |
| `plugin::runtime` | `host` | Axis two, format: the `Runtime` contract, plus one submodule per format |
| `plugin::runtime::cdylib` | `runtime-cdylib` | The cdylib `Runtime`: the vtable handed out, the callbacks wrapped back |
| `host` | `host` | Application-side runtime: dependency resolution, lifecycle. Discovery is `load-file` |
| `sdk` | — | The plugin-author prelude |
| `sdk::cdylib` | `compile-cdylib` | Writing a Rust plugin and building it as a `.so`: `export_plugin!` and the host vtable it calls |
| `testing` | `testing` | Test harness: sandboxes, fixture plugins, hook assertions |

## Versioning & publishing

One crate, `0.x` (minor bump = breaking). Version is bumped once before a release tag, not per
commit.

The **ABI is the compatibility boundary for other people's builds**: a changed `abi` module layout
means every plugin ever built must be rebuilt, and two semver-incompatible copies of `plugx` in one
process means two registries and silently dead hooks. Break either only when there is no
alternative, and never for convenience.

## Architecture invariants

These are not style preferences. Breaking one is a crash or a silent no-op in somebody else's
process.

- **One global, and it holds a pointer, not a registry.** `registry::REGISTRY` is the only `static` in
  the crate holding state: the registry one `Host` leaked, installed when it is built and cleared
  when it is dropped. A plugin `.so` links its own copy of this crate and so gets its own copy of
  that slot, which nothing ever fills — so `run` inside a plugin returns `Error::NotInitialized` instead of
  swallowing the call into a private, permanently empty table. Adding a second global, or filling
  the slot from anywhere but `Host`, brings back the silent dead registry this design removed.
- **One live `Host` per process.** `Host::new` claims the slot and fails with `Error::HostExists`
  while another host holds it; `Drop` gives it back. The registry itself stays leaked, because a
  plugin may still hold the pointer.
- **No lock is ever held while a callback or an exported function executes.** Dispatch and
  `plugin_call` snapshot the table (brief read lock), release, *then* invoke. Holding across
  execution deadlocks the moment a callback registers another callback or calls a second plugin.
- **The stop sequence is drain → quiesce → `drop_fn` → `Plugin::stop` → leak.** One `drain` takes
  both the callbacks *and* the exported functions out, and `Drained::wait` quiesces both. Calling
  `Plugin::stop` before in-flight dispatches and calls have drained means code runs against a
  plugin that already tore down its state.
- **Never `dlclose`.** A stopped plugin's library stays mapped for the life of the process. Code
  reload dlopens a fresh copy at a versioned path. Leaking is always safe; unloading is not.
- **Nothing Rust-owned crosses the ABI.** Only `Str` (ptr + len, which the receiver copies
  immediately) and opaque handles freed by whoever created them. A plugin cdylib has its own
  allocator; a `String` allocated on one side and freed on the other is undefined behaviour.
- **Never resolve symbols back into the host.** No `-rdynamic`, no `RTLD_GLOBAL`. Everything a
  plugin needs arrives as function pointers in `Context`. Symbol lookback does not exist on Windows.
- **A plugin's context is rebuilt per call and stored nowhere.** Every lifecycle symbol takes the
  host's `abi::Context`; the SDK turns it into a `Context` for the length of that call and drops
  it. There is no entry point, no vtable coming back, no instance pointer and no install step — a
  plugin that wants one on a background thread keeps its own copy.
- **A plugin's name is its identity, and its filename is its name.** The *runtime* parses it, by
  its own format's rules, because the extension is what chose that runtime: `libauth.so` is `auth`,
  leaked once at load, compared as a `&'static str`, unique per host. Registrations are tagged with
  it and `plugin::function` addresses through it. One library is one plugin.
- **A runtime stamps the name into `Info::name`, and that is how a host learns it.** `build` hands
  back plugins and nothing else; the host asks each one for its `Info` through a context named `""`,
  because the name is what it is asking for. A runtime that forgets to stamp produces plugins the
  host refuses as unnamed.
- **A plugin exports one symbol per operation.** `plugx_abi_version`, `plugx_info`, `plugx_start`,
  `plugx_reload`, `plugx_stop`. The host resolves them by name after the version check. Adding an
  operation appends a symbol; a host treats a missing one as unsupported.
- **`host_data` is per plugin, not per host.** The cdylib runtime leaks one `PluginData` — registry
  plus leaked name — for each library it loads and puts that pointer in the context. It is what
  answers "who is calling", so no ABI entry takes an `owner`: a plugin cannot tag a registration
  with somebody else's name, because it never gets to say a name at all.
- **A failure travels with the call that failed.** Whoever fails writes a message into the
  `error_out` it was handed and the other side copies it out on return, in both directions. There
  is no "ask me why" entry point: a second crossing to fetch a message the first one already had is
  a race waiting to happen, and it was `plugx_last_error`.

## `unsafe`

Confined to the modules that touch a plugin boundary: `abi::cdylib`, `plugin::runtime::cdylib` and
`sdk::cdylib`. Inside the last of those, `sdk::cdylib::host` calls the host's function pointers
from inside a loaded library; its whole unsafe surface rests on a single contract, established
once by `HostRef::new` (an `unsafe fn`): the vtable is valid and outlives the process.

`value`, `error`, `context`, `hook`, `plugin` (its own files) and `host` must contain no `unsafe`. So must
`registry`, with one exception written into it: dereferencing the slot in `run`, whose invariant is
that the only non-null value the slot ever holds is a leaked `&'static Registry`. This
used to be `#![forbid(unsafe_code)]` on the crates that became those modules; a single crate cannot
forbid per-module, so it is now a review rule rather than a compiler-enforced one. An `unsafe` block
appearing in one of them is a defect.

Every `unsafe` block carries a `// SAFETY:` comment naming the invariant the caller must uphold and
who upholds it. A block without one does not merge.

## Testing

No `#[cfg(test)]` blocks in `src/` — ever. All tests live in `tests/`. Naming: `lib.rs` tests →
`tests/all.rs`; `src/x/mod.rs` → `tests/x.rs`; `src/a/b.rs` → `tests/a_b.rs`. A test needing a
private item with no public path is deleted, not kept inline and not exposed via a new `pub`.

Anything concurrent (dispatch during registration, stop during dispatch, a call in flight while its
plugin stops) gets a test that actually spawns threads and joins them, not a single-threaded
approximation.

`tests/abi.rs` compiles `examples/c_echo_plugin.c` against `include/plugx.h` with the system C
compiler and drives it through the ordinary host API. The header is part of the ABI contract:
change a `repr(C)` struct in `src/abi/cdylib` and it changes too, or the test stops meaning anything.

Until the rest of the written tests land, `make examples && ./bin/demo bin` is the end-to-end
check: it scans a
directory, refuses a second host and a duplicate name, fires a hook with `plugx::run` whose callback
calls the other plugin and the host, walks every failure mode, and proves the slot is free again
once the host is dropped.

## Code style conventions

- **A list is named `_list`; a map is named with a plural.** The suffix says which of the two a
  name holds, so that a reader knows the shape without going to look:
  - **`Vec`, slice, array, iterator → `<thing>_list`.** `artifact_list`, `plugin_list`,
    `entry_list`, `schema_list`, `extension_list`, `dependency_list` — **never** `artifacts`,
    `plugins`, `entries`, `dependencies`. A trailing `s` is one character from the singular and
    gets read as the singular; `_list` never does.
  - **Map, table, anything keyed → the plural.** `hooks`, `apis`, `states`: a keyed collection is
    addressed one key at a time, so the plural is what it reads as at the use site.
  - A name holding **one** struct takes neither, even when that struct contains a collection:
    `symbols` is a record of five function pointers, not five of anything.
  - **The one exception is `bytes`**, along with std's `as_bytes` / `to_bytes` family: binary data
    is a mass noun, not a list of `u8` values, and std has already settled its spelling.

  This is not per-module. A `Vec` under a plural name is a defect in `src/`, `tests/`, `examples/`
  and the C header alike.
- **Plain `for` loops over iterator method chains.** Prefer a `for` loop to
  `.map`/`.filter`/`.fold`/`.collect` chains. (When you do index a slice, still use `for x in &xs` /
  `.iter().enumerate()` to satisfy `needless_range_loop`.)
- **`match` over combinators for `Result`.** Use an explicit `match` instead of `map_err`,
  `and_then`, `or_else`, etc.
- **`if let Some(...)` over combinators for `Option`.** Use `if let` / `match` instead of `map`,
  `and_then`, `unwrap_or_else`, etc.
- **Don't extract single-use helpers.** If a function is called from exactly one place, inline it.

## Lint & style conventions

- **No `#[allow(...)]` anywhere** — fix the root cause instead of suppressing a lint. **Exception:**
  `#[allow(unused_mut)]` is permitted on functions whose `mut` binding is needed only when certain
  cargo features are active (`#[cfg(feature = "...")]`). In that case the mutability is real but
  conditionally compiled away, so `#[allow(unused_mut)]` is the correct fix. Do not use it for any
  other lint or any other reason.
- **`make clippy` is the gate** — it runs
  `cargo clippy --all-features --all-targets --no-deps -- -D warnings`, so warnings
  (including in tests and examples) fail the build.
- **`clippy::type_complexity`** — extract a named `pub type` alias rather than spelling out nested
  generic types in signatures.
- **`clippy::result_large_err`** — keep error enums small enough to return by value without `Box`.
  Shrink fields (e.g. `Option<NonZeroU32>` instead of `Option<usize>`) instead of boxing the error
  or allowing the lint.
- **`needless_range_loop`** — iterate with `for x in &xs` / `.iter().enumerate()`, not
  `for i in 0..xs.len()`.
- **`vec_init_then_push`** — when conditional (`#[cfg]`) construction prevents a `vec![…]` literal,
  use `Vec::extend([…])` rather than repeated `push`.

## Logging conventions

The crate carries **both** the `logging` (→ `log`) and `tracing` features. Choosing between the
two is a compile-time decision made with `cfg_if`, never a runtime one. **Never use `error!`** —
this is a library; it propagates errors to the caller and lets the application decide what is an
error.

### Level guide

| Level | When to use |
|-------|-------------|
| `info` | Important success event (plugin started, library loaded, host booted, config validated) |
| `warn` | Intentionally tolerated failure (an optional plugin failed to load, a stop timed out and the plugin was left drained) |
| `debug` | Before attempting something important — include the inputs/params that affect the outcome |
| `trace` | After completing a low-level operation — include rich detail about what was produced |

### Format rules

Every log call starts with a `msg` field whose value begins with a capital letter. Additional
structured fields follow as `key=value` pairs. Use the shared field names below so logs from
different modules join up:

`plugin` (name), `hook`, `function`, `registration`, `loader`, `path`, `abi`, `priority`,
`callback_count`, `function_count`, `kind` (`transform` / `observe`), `flow`
(`continue` / `stop`), `generation`, `elapsed_us`.

There is no `plugin_id`: the name *is* the id.

**`tracing` feature:**
```rust
tracing::info!(msg = "Started plugin", plugin = name);
tracing::warn!(msg = "Skipped optional plugin", plugin = name, error = ?e);
tracing::debug!(msg = "Dispatching hook", hook = hook.name(), callback_count = entries.len());
tracing::trace!(msg = "Callback returned", hook = hook.name(), plugin = name, flow = ?flow);
```

**`logging` feature** — mirror the same key=value pairs in the format string:
```rust
log::info!("msg=\"Started plugin\" plugin={name}");
log::warn!("msg=\"Skipped optional plugin\" plugin={name} error={e:?}");
log::debug!("msg=\"Dispatching hook\" hook={} callback_count={}", hook.name(), entries.len());
log::trace!("msg=\"Callback returned\" hook={} plugin={name} flow={flow:?}", hook.name());
```

### Pattern

Wrap every call in `cfg_if!` — check `tracing` first, then `logging`:

```rust
use cfg_if::cfg_if;

cfg_if! {
    if #[cfg(feature = "tracing")] {
        tracing::debug!(msg = "Loading plugin", plugin = name, path = ?path);
    } else if #[cfg(feature = "logging")] {
        log::debug!("msg=\"Loading plugin\" plugin={name} path={path:?}");
    }
}
```

## Spans (`tracing` feature only)

Spans are what make plugx's logs actually readable: a hook dispatch fans out into N plugins, and
without a span nobody can tell which callback emitted which line. **When `tracing` is enabled,
every operation that can emit more than one log line, or that calls into plugin code, opens a
span.**

The codebase is synchronous, so spans are entered with a guard — `let _span = ….enter();` — never
`.instrument()`.

### Span level guide

| Span level | Wraps |
|------------|-------|
| `info_span` | Host boot; a whole plugin lifecycle transition (`plugin.load`, `plugin.start`, `plugin.reload`, `plugin.stop`) |
| `debug_span` | One loader attempt; one config validation; one dependency-resolution pass |
| `trace_span` | One hook dispatch that has callbacks; one individual callback invocation |

### Span naming and fields

Names are dotted lowercase and match the operation, not the function: `host.boot`, `plugin.load`,
`plugin.start`, `plugin.reload`, `plugin.stop`, `hook.dispatch`, `hook.callback`.

Fields go on the **span**, not repeated on every event inside it. Put the identity of the thing
being operated on in the span (`plugin`, `hook`, `loader`) and keep the events to what changed.

### Pattern

```rust
cfg_if! {
    if #[cfg(feature = "tracing")] {
        let _span = tracing::info_span!("plugin.start", plugin = name).entered();
        tracing::debug!(msg = "Validating configuration");
    } else if #[cfg(feature = "logging")] {
        log::debug!("msg=\"Validating configuration\" plugin={name}");
    }
}
```

Note the asymmetry, and it is deliberate: **under `logging` there is no span context, so the
identifying fields must be repeated on every event.** When you add a field to a span, add it to
each mirrored `log::` line in that scope.

### Rules

- **Never open a span on the empty-hook fast path.** `Context::run` proves a hook has no callbacks
  with one relaxed atomic load; a span there would cost more than the entire dispatch. The
  `hook.dispatch` span opens only after that check passes.
- **A span wraps the error return too.** Open it before the fallible work so a caller's error log
  still carries the plugin and hook that produced it.
- **No `#[tracing::instrument]`.** It hides the cost of field capture, cannot be mirrored to
  `log`, and names spans after functions rather than operations. Write the span explicitly.
- **Never log payload contents.** A hook's `Value` may be large and may hold secrets. Log its
  `kind`, its length, or the map's key count — never the value itself, at any level.
