/* plugx C ABI, version 2.0.0.
 *
 * Everything a plugin written in C needs, and nothing else: there is no plugx library to link
 * against. A plugin is a shared object that defines the six `plugx_*` symbols at the bottom of
 * this file; the host resolves them by name after checking the version.
 *
 * The rules this header encodes:
 *
 *   - Every vtable starts with `size`, naming its own byte length. Check it before reading a field
 *     an older host may not have written. Fields are only ever appended.
 *   - Nothing owned by one side is freed by the other. Strings cross as `PlugxStr`, which the
 *     receiver copies immediately; value trees cross as opaque handles, built and released through
 *     the host's `PlugxValueApi`.
 *   - Never resolve symbols back into the host. Everything reachable arrives as function pointers
 *     inside `PlugxContext`.
 *
 * Build a plugin with, for example:
 *
 *     cc -std=c11 -shared -fPIC -Iinclude -o libmine.so mine.c
 */

#ifndef PLUGX_H
#define PLUGX_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ---- primitives -------------------------------------------------------------------------- */

/* The ABI this header describes. A host loads a plugin only when `major` matches its own and
 * `minor` is no higher. */
#define PLUGX_ABI_MAJOR 2u
#define PLUGX_ABI_MINOR 0u
#define PLUGX_ABI_PATCH 0u

typedef struct PlugxAbiVersion {
  uint32_t major;
  uint32_t minor;
  uint32_t patch;
} PlugxAbiVersion;

/* A borrowed string: a pointer and a length, with no NUL terminator, valid only for the call it was
 * passed to. Copy it if you need it afterwards.
 *
 * Pointer plus length rather than `const char *`, because the other side's strings are not
 * NUL-terminated and may contain an interior NUL. UTF-8 is expected but not enforced: the host
 * checks the bytes it receives and refuses the call if they are not. */
typedef struct PlugxStr {
  const uint8_t *ptr;
  size_t len;
} PlugxStr;

/* The result of a call across the boundary. Anything negative is a failure. */
typedef int32_t PlugxStatus;
#define PLUGX_OK 0
#define PLUGX_STOP 1
#define PLUGX_ERROR (-1)
#define PLUGX_UNSUPPORTED (-2)
#define PLUGX_INCOMPATIBLE (-3)

/* The kind tag returned by `PlugxValueApi::kind`. */
typedef uint8_t PlugxKind;
#define PLUGX_KIND_BOOL 0
#define PLUGX_KIND_INT 1
#define PLUGX_KIND_FLOAT 2
#define PLUGX_KIND_STR 3
#define PLUGX_KIND_LIST 4
#define PLUGX_KIND_MAP 5

/* Log levels for `PlugxHostApi::log`, matching the `log` crate. */
#define PLUGX_LOG_ERROR 1
#define PLUGX_LOG_WARN 2
#define PLUGX_LOG_INFO 3
#define PLUGX_LOG_DEBUG 4
#define PLUGX_LOG_TRACE 5

/* ---- values ------------------------------------------------------------------------------ */

/* An opaque value tree owned by the host. A plugin never sees the layout.
 *
 * Handles come in two flavours, and confusing them is a double free:
 *   - borrowed: what a callback is handed, and what `list_get` / `map_get` return. Valid until the
 *     call returns. Never release one.
 *   - owned: what the `new_*` constructors return. Release it, or hand it to `map_set` /
 *     `list_push`, which take ownership. */
typedef struct PlugxValue PlugxValue;

typedef struct PlugxValueApi {
  size_t size;

  PlugxKind (*kind)(const PlugxValue *value);

  PlugxStatus (*get_bool)(const PlugxValue *value, bool *out);
  PlugxStatus (*get_int)(const PlugxValue *value, int64_t *out);
  PlugxStatus (*get_float)(const PlugxValue *value, double *out);
  PlugxStatus (*get_str)(const PlugxValue *value, PlugxStr *out);

  PlugxStatus (*list_len)(const PlugxValue *value, size_t *out);
  PlugxValue *(*list_get)(PlugxValue *value, size_t index);
  PlugxStatus (*list_push)(PlugxValue *value, PlugxValue *item);

  PlugxStatus (*map_len)(const PlugxValue *value, size_t *out);
  PlugxStatus (*map_key_at)(const PlugxValue *value, size_t index, PlugxStr *out);
  PlugxValue *(*map_get)(PlugxValue *value, PlugxStr key);
  PlugxStatus (*map_set)(PlugxValue *value, PlugxStr key, PlugxValue *item);
  PlugxStatus (*map_remove)(PlugxValue *value, PlugxStr key);

  PlugxStatus (*set_bool)(PlugxValue *value, bool item);
  PlugxStatus (*set_int)(PlugxValue *value, int64_t item);
  PlugxStatus (*set_float)(PlugxValue *value, double item);
  PlugxStatus (*set_str)(PlugxValue *value, PlugxStr item);
  PlugxStatus (*set_list)(PlugxValue *value);
  PlugxStatus (*set_map)(PlugxValue *value);

  PlugxValue *(*new_bool)(bool item);
  PlugxValue *(*new_int)(int64_t item);
  PlugxValue *(*new_float)(double item);
  PlugxValue *(*new_str)(PlugxStr item);
  PlugxValue *(*new_list)(void);
  PlugxValue *(*new_map)(void);

  void (*release)(PlugxValue *value);
} PlugxValueApi;

/* ---- what a plugin registers ------------------------------------------------------------- */

/* A hook callback. `data` is borrowed for the call: a transform may rewrite it, an observer must
 * not. Return PLUGX_STOP to end the dispatch. */
typedef PlugxStatus (*PlugxCallbackFn)(void *user_data, PlugxValue *data);

/* An exported function. `args` is borrowed; write an owned handle to `out`. On PLUGX_ERROR you may
 * write your error value to `out` instead, and the caller receives it. */
typedef PlugxStatus (*PlugxApiCallFn)(void *user_data, const PlugxValue *args, PlugxValue **out);

/* Frees a `user_data`. Called once, after the record is out of the host's table and everything
 * in flight has finished, while this library is still mapped. NULL if there is nothing to free. */
typedef void (*PlugxDropFn)(void *user_data);

typedef struct PlugxCallback {
  PlugxCallbackFn call;
  void *user_data;
  PlugxDropFn drop;
} PlugxCallback;

typedef struct PlugxApiFunction {
  PlugxApiCallFn call;
  void *user_data;
  PlugxDropFn drop;
} PlugxApiFunction;

/* ---- what the host offers ----------------------------------------------------------------- */

/* Every entry takes `host_data` first: the opaque pointer from the context, which is how the host
 * finds its own registry. Registration entries take the plugin's own name as `owner` — the name is
 * the identity in this ABI, and there is no separate id. */
typedef struct PlugxHostApi {
  size_t size;

  const PlugxValueApi *value;

  PlugxStatus (*register_transform)(void *host_data, PlugxStr owner, PlugxStr hook,
                                    int32_t priority, PlugxCallback callback, uint64_t *out_id);
  PlugxStatus (*register_observe)(void *host_data, PlugxStr owner, PlugxStr hook,
                                  int32_t priority, PlugxCallback callback, uint64_t *out_id);
  PlugxStatus (*unregister)(void *host_data, PlugxStr owner, uint64_t id);

  PlugxStatus (*run)(void *host_data, PlugxStr hook, PlugxValue *data);

  void (*log)(void *host_data, uint8_t level, PlugxStr message);
  PlugxStatus (*last_error)(void *host_data, PlugxStr *out);

  PlugxStatus (*export_fn)(void *host_data, PlugxStr owner, PlugxStr name,
                           PlugxApiFunction function, uint64_t *out_id);
  PlugxStatus (*unexport)(void *host_data, PlugxStr owner, uint64_t id);

  PlugxStatus (*plugin_call)(void *host_data, PlugxStr target, const PlugxValue *args,
                             PlugxValue **out);
  PlugxStatus (*host_call)(void *host_data, PlugxStr name, const PlugxValue *args,
                           PlugxValue **out);
} PlugxHostApi;

/* What the host passes to every symbol below. Borrowed for the call — but `host`, `host_data` and
 * the bytes behind `plugin_name` outlive the process, because a host never unloads a plugin. */
typedef struct PlugxContext {
  size_t size;
  PlugxAbiVersion abi;
  const PlugxHostApi *host;
  void *host_data;
  PlugxStr plugin_name;
} PlugxContext;

/* ---- what a plugin must export ------------------------------------------------------------ */

/* Called first, before anything else in the library, so an incompatible plugin can be found and
 * abandoned before it has been given anything. */
PlugxAbiVersion plugx_abi_version(void);

/* Describe yourself: write an owned map to `out` with a "version" string ("1.0.0") and, if you
 * like, a "description" string, a "config_spec" tree and a "dependencies" list. */
PlugxStatus plugx_info(const PlugxContext *context, PlugxValue **out);

/* Come up: register hook callbacks and export functions, all through `context`. */
PlugxStatus plugx_start(const PlugxContext *context, const PlugxValue *config);

/* Take a new configuration. Return PLUGX_UNSUPPORTED and the host stops and starts you instead. */
PlugxStatus plugx_reload(const PlugxContext *context, const PlugxValue *old_config,
                         const PlugxValue *new_config);

/* Tear down. Everything you registered is already out of the host's registry and quiesced. */
PlugxStatus plugx_stop(const PlugxContext *context);

/* Why the last call into this plugin failed. The slice must stay valid until the next call. */
PlugxStatus plugx_last_error(PlugxStr *out);

/* Borrow a NUL-terminated C string as a PlugxStr. The terminator is not included. */
static inline PlugxStr plugx_str(const char *text) {
  PlugxStr borrowed;
  borrowed.ptr = (const uint8_t *)text;
  borrowed.len = 0;
  while (text[borrowed.len] != '\0') {
    borrowed.len++;
  }
  return borrowed;
}

#ifdef __cplusplus
}
#endif

#endif /* PLUGX_H */
