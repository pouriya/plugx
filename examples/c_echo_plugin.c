/* A plugx plugin written in plain C.
 *
 * It exports one function and registers one hook callback, and from inside that callback it calls
 * back into the application — the same three things the Rust examples do, with no Rust anywhere in
 * this file and nothing to link against but `include/plugx.h`.
 *
 *     cc -std=c11 -shared -fPIC -Iinclude -o libc_echo_plugin.so examples/c_echo_plugin.c
 *
 * The host names a plugin after its file, so that builds a plugin called `c_echo_plugin`.
 */

#include "plugx.h"

#include <ctype.h>
#include <stdio.h>
#include <string.h>

/* What this plugin needs to reach the host, copied out of the context in `plugx_start` and handed
 * back to each callback as its `user_data`. The host passes the context on every call; this is a
 * copy kept only because a callback is invoked later, out of any call of ours. */
typedef struct Reach {
  const PlugxHostApi *host;
  void *host_data;
  const PlugxValueApi *value;
  PlugxSlice name;
} Reach;

static Reach REACH;

/* Why the last call into this plugin failed. Static, and so valid until the next call, which is
 * exactly what the ABI asks for. */
static char LAST_ERROR[256];

static PlugxStatus fail(const char *message) {
  snprintf(LAST_ERROR, sizeof(LAST_ERROR), "%s", message);
  return PLUGX_ERROR;
}

/* ---- the hook callback ---------------------------------------------------------------------
 *
 * Runs on every `request.headers` dispatch, stamps the payload, and calls the application's own
 * `stamp` function while the dispatch is still in flight. */
static PlugxStatus on_request_headers(void *user_data, PlugxValue *data) {
  Reach *reach = (Reach *)user_data;
  if (reach->value->kind(data) != PLUGX_KIND_MAP) {
    return PLUGX_OK;
  }

  PlugxValue *seen = reach->value->new_str(plugx_str("c_echo_plugin"));
  if (seen == NULL) {
    return PLUGX_ERROR;
  }
  /* `map_set` takes ownership, so `seen` must not be released here. */
  reach->value->map_set(data, plugx_str("seen-by-c"), seen);

  PlugxValue *args = reach->value->new_map();
  if (args == NULL) {
    return PLUGX_ERROR;
  }
  PlugxValue *stamped = NULL;
  PlugxStatus status = reach->host->host_call(reach->host_data, plugx_str("stamp"), args, &stamped);
  reach->value->release(args);
  if (status != PLUGX_OK) {
    if (stamped != NULL) {
      reach->value->release(stamped);
    }
    return PLUGX_ERROR;
  }
  reach->value->map_set(data, plugx_str("c-stamp"), stamped);

  return PLUGX_OK;
}

/* ---- the exported function ------------------------------------------------------------------
 *
 * `c_echo_plugin::shout` — uppercase a string. Anyone can call it: the application, another
 * plugin, or a callback in the middle of a dispatch. */
static PlugxStatus shout(void *user_data, const PlugxValue *args, PlugxValue **out) {
  Reach *reach = (Reach *)user_data;

  PlugxSlice text;
  if (reach->value->kind(args) != PLUGX_KIND_STR ||
      reach->value->get_str(args, &text) != PLUGX_OK) {
    /* A plugin's own failure crosses as a free-form value, written to `out`. */
    *out = reach->value->new_str(plugx_str("shout wants a string"));
    return PLUGX_ERROR;
  }

  char buffer[256];
  size_t len = text.len < sizeof(buffer) - 1 ? text.len : sizeof(buffer) - 1;
  for (size_t index = 0; index < len; index++) {
    buffer[index] = (char)toupper((unsigned char)text.ptr[index]);
  }
  buffer[len] = '\0';

  PlugxSlice shouted;
  shouted.ptr = (const uint8_t *)buffer;
  shouted.len = len;
  *out = reach->value->new_str(shouted);
  if (*out == NULL) {
    return PLUGX_ERROR;
  }
  return PLUGX_OK;
}

/* ---- the lifecycle symbols ------------------------------------------------------------------ */

PlugxAbiVersion plugx_abi_version(void) {
  PlugxAbiVersion version;
  version.major = PLUGX_ABI_MAJOR;
  version.minor = PLUGX_ABI_MINOR;
  version.patch = PLUGX_ABI_PATCH;
  return version;
}

/* Everything below starts the same way: check the context is one this build understands. */
static bool usable(const PlugxContext *context) {
  if (context == NULL || context->size < sizeof(PlugxContext) || context->host == NULL) {
    return false;
  }
  if (context->abi.major != PLUGX_ABI_MAJOR) {
    return false;
  }
#if PLUGX_ABI_MINOR > 0
  /* A host older than the minor this plugin was written against has not written every field it
   * reads. At minor 0 there is nothing older, so there is nothing to check. */
  if (context->abi.minor < PLUGX_ABI_MINOR) {
    return false;
  }
#endif
  return true;
}

PlugxStatus plugx_info(const PlugxContext *context, PlugxValue **out) {
  if (!usable(context) || out == NULL) {
    return PLUGX_INCOMPATIBLE;
  }
  const PlugxValueApi *value = context->host->value;

  PlugxValue *info = value->new_map();
  if (info == NULL) {
    return fail("could not allocate the info map");
  }
  value->map_set(info, plugx_str("version"), value->new_str(plugx_str("1.0.0")));
  value->map_set(info, plugx_str("description"),
                 value->new_str(plugx_str("Shouts strings, from C")));
  *out = info;
  return PLUGX_OK;
}

PlugxStatus plugx_start(const PlugxContext *context, const PlugxValue *config) {
  (void)config;
  if (!usable(context)) {
    return PLUGX_INCOMPATIBLE;
  }

  REACH.host = context->host;
  REACH.host_data = context->host_data;
  REACH.value = context->host->value;
  REACH.name = context->plugin_name;

  PlugxCallback callback;
  callback.call = on_request_headers;
  callback.user_data = &REACH;
  callback.drop = NULL; /* `REACH` is static; there is nothing to free. */

  uint64_t registration = 0;
  if (REACH.host->register_transform(REACH.host_data, REACH.name, plugx_str("request.headers"), 20,
                                     callback, &registration) != PLUGX_OK) {
    return fail("the host refused the callback");
  }

  PlugxApiFunction function;
  function.call = shout;
  function.user_data = &REACH;
  function.drop = NULL;

  uint64_t exported = 0;
  if (REACH.host->export_fn(REACH.host_data, REACH.name, plugx_str("shout"), function, &exported) !=
      PLUGX_OK) {
    return fail("the host refused the export");
  }

  REACH.host->log(REACH.host_data, PLUGX_LOG_INFO, plugx_str("msg=\"Started\" plugin=c"));
  return PLUGX_OK;
}

PlugxStatus plugx_reload(const PlugxContext *context, const PlugxValue *old_config,
                         const PlugxValue *new_config) {
  (void)context;
  (void)old_config;
  (void)new_config;
  /* Nothing to reconfigure, so let the host stop and start us instead. */
  return PLUGX_UNSUPPORTED;
}

PlugxStatus plugx_stop(const PlugxContext *context) {
  if (!usable(context)) {
    return PLUGX_INCOMPATIBLE;
  }
  /* The host has already taken the callback and the function out of its tables and waited for
   * everything in flight, so there is nothing to unregister here. */
  return PLUGX_OK;
}

PlugxStatus plugx_last_error(PlugxSlice *out) {
  if (out == NULL) {
    return PLUGX_ERROR;
  }
  out->ptr = (const uint8_t *)LAST_ERROR;
  out->len = strlen(LAST_ERROR);
  return PLUGX_OK;
}
