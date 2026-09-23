/* plugx C ABI, version 3.0.0.
 *
 * Everything a plugin written in C needs, and nothing else: there is no plugx library to link
 * against. A plugin is a shared object that defines the five `plugx_*` symbols at the bottom of
 * this file; the host resolves them by name after checking the version.
 *
 * The rules this header encodes:
 *
 *   - Every vtable starts with `size`, naming its own byte length. Check it before reading a field
 *     an older host may not have written. Fields are only ever appended.
 *   - Nothing owned by one side is freed by the other. Strings cross as `PlugxStr`, which the
 *     receiver copies immediately; value trees cross as opaque handles, built and released through
 *     the host's `PlugxValueApi`.
 *   - A failure travels with the call that failed. Whoever fails writes a message into the
 *     `error_out` it was handed, and the other side copies it out the moment the call returns.
 *     Nobody calls back to ask.
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
#define PLUGX_ABI_MAJOR 3u
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
#define PLUGX_CONTINUE_ERROR 2
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
 * not.
 *
 * A callback answers two questions at once — where the dispatch goes next, and whether this
 * callback failed — so there are four things to return:
 *
 *   PLUGX_OK                                     carry on to the next callback
 *   PLUGX_STOP                                   end the dispatch here
 *   host->continue_with_error(host_data, why)    carry on, having failed
 *   host->stop_with_error(host_data, why)        end the dispatch, having failed
 *
 * The two helpers record the message and hand back the code to return, so the whole of it is
 * `return host->stop_with_error(host_data, plugx_str("redis is down"));`. Failing without stopping
 * is logged against your plugin and goes no further; failing and stopping is what whoever fired
 * the hook receives. */
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

/* Every entry takes `host_data` first: the opaque pointer from the context. It is per *plugin*, not
 * per host — a host hands a different one to each library it loads — so the host already knows who
 * is calling. That is why nothing here names an owner: your registrations are tagged with the name
 * the host loaded you under, which you cannot spell wrong and cannot spell as somebody else.
 *
 * Every entry that can fail also takes an `error_out` last, and writes a message into it before
 * returning anything but PLUGX_OK. Copy it immediately; it borrows a buffer the host reuses on your
 * next call. Pass NULL if you do not care, and initialise it to a zero PlugxStr otherwise, because
 * a host with nothing to say leaves it alone. `unregister` and `unexport` have none: their whole
 * answer is whether the registration was there. */
typedef struct PlugxHostApi {
  size_t size;

  const PlugxValueApi *value;

  PlugxStatus (*register_transform)(void *host_data, PlugxStr hook, int32_t priority,
                                    PlugxCallback callback, uint64_t *out_id, PlugxStr *error_out);
  PlugxStatus (*register_observe)(void *host_data, PlugxStr hook, int32_t priority,
                                  PlugxCallback callback, uint64_t *out_id, PlugxStr *error_out);
  PlugxStatus (*unregister)(void *host_data, uint64_t id);

  PlugxStatus (*run)(void *host_data, PlugxStr hook, PlugxValue *data, PlugxStr *error_out);

  void (*log)(void *host_data, uint8_t level, PlugxStr message);

  /* Only from inside a hook callback, only on the way out: record why this callback failed, and
   * take back the code to return. See PlugxCallbackFn. */
  PlugxStatus (*continue_with_error)(void *host_data, PlugxStr message);
  PlugxStatus (*stop_with_error)(void *host_data, PlugxStr message);

  PlugxStatus (*export_fn)(void *host_data, PlugxStr name, PlugxApiFunction function,
                           uint64_t *out_id, PlugxStr *error_out);
  PlugxStatus (*unexport)(void *host_data, uint64_t id);

  PlugxStatus (*plugin_call)(void *host_data, PlugxStr target, const PlugxValue *args,
                             PlugxValue **out, PlugxStr *error_out);
  PlugxStatus (*host_call)(void *host_data, PlugxStr name, const PlugxValue *args,
                           PlugxValue **out, PlugxStr *error_out);
} PlugxHostApi;

/* What the host passes to every symbol below. Borrowed for the call — but `host`, `host_data` and
 * the bytes behind `plugin_name` outlive the process, because a host never unloads a plugin.
 *
 * `host_data` is yours alone: it is how the host knows which plugin is calling. Hand it back
 * unchanged and never hand out a copy. */
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

/* Each of these takes an `error_out` last, and the rule is the mirror of the host's: write a
 * message into it before returning anything but PLUGX_OK, and point it at storage that stays valid
 * until the next call into you — a `static` buffer is enough, because the host copies it out
 * straight away. It may be NULL. */

/* Describe yourself: write an owned map to `out` with a "version" string ("1.0.0") and, if you
 * like, a "description" string, a "config_spec" tree and a "dependencies" list. */
PlugxStatus plugx_info(const PlugxContext *context, PlugxValue **out, PlugxStr *error_out);

/* Come up: register hook callbacks and export functions, all through `context`. */
PlugxStatus plugx_start(const PlugxContext *context, const PlugxValue *config,
                        PlugxStr *error_out);

/* Take a new configuration. Return PLUGX_UNSUPPORTED and the host stops and starts you instead. */
PlugxStatus plugx_reload(const PlugxContext *context, const PlugxValue *old_config,
                         const PlugxValue *new_config, PlugxStr *error_out);

/* Tear down. Everything you registered is already out of the host's registry and quiesced. */
PlugxStatus plugx_stop(const PlugxContext *context, PlugxStr *error_out);

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
