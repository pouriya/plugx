//! A plugin whose hook callback calls another plugin and the host.
//!
//! This is the case the redesign exists for: the callback is handed its own context, so from
//! inside a dispatch it can reach `echo::reverse` and the application's own `stamp`.

use plugx::sdk::prelude::*;

#[derive(Default)]
struct Caller;

impl Plugin for Caller {
    fn info(&self, _context: &Context) -> Result<Info> {
        Ok(
            Info::new(Version::new(1, 0, 0), "Calls echo from a callback")
                .with_dependency(Dependency::new("echo_plugin", Version::new(1, 0, 0))),
        )
    }

    fn start(&self, context: &Context, _config: &Value) -> Result<()> {
        context.on_transform("request.headers", 10, |own: &Context, data: &mut Value| {
            let path = match data.get_path("path") {
                Some(path) => path.clone(),
                None => Value::Str(String::new()),
            };
            let reversed = own.plugin_call("echo_plugin::reverse", path)?;
            let stamped = own.host_call("stamp", Value::map())?;
            if let Some(map) = data.as_map_mut() {
                map.insert("reversed", reversed);
                map.insert("stamp", stamped);
            }
            Ok(Flow::Continue)
        })?;

        // Exported after the callback, so the demo can check `unexport` and `Duplicate` too.
        context.export("ping", |own: &Context, _args: Value| {
            Ok(Value::Str(format!("pong from {}", own.name())))
        })?;
        Ok(())
    }

    fn stop(&self, _context: &Context) -> Result<()> {
        Ok(())
    }
}

plugx::export_plugin!(Caller);
