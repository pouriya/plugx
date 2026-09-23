//! A plugin that exports one function and watches one hook.
//!
//! Build it as a `cdylib` and load it with `examples/demo.rs`.

use plugx::sdk::prelude::*;

#[derive(Default)]
struct Echo;

impl Plugin for Echo {
    fn info(&self, _context: &Context) -> Result<Info> {
        Ok(Info::new(
            Version::new(1, 0, 0),
            "Reverses strings and counts dispatches",
        ))
    }

    fn start(&self, context: &Context, _config: &Value) -> Result<()> {
        // Anyone can now call `echo::reverse`.
        context.export("reverse", |_: &Context, args: Value| match args.as_str() {
            Some(text) => Ok(Value::Str(text.chars().rev().collect())),
            None => Err(plugx::Error::Failed {
                plugin: "echo_plugin".into(),
                error: Box::new(Value::Str("reverse wants a string".to_string())),
            }),
        })?;

        // And a callback that stamps every dispatch, to prove registration works from inside a
        // shared library with no global anywhere.
        context.on_transform("request.headers", 0, |own: &Context, data: &mut Value| {
            if let Some(map) = data.as_map_mut() {
                map.insert("seen-by", Value::Str(own.name().to_string()));
            }
            Flow::Continue(Ok(()))
        })?;
        Ok(())
    }

    fn stop(&self, _context: &Context) -> Result<()> {
        Ok(())
    }
}

plugx::export_plugin!(Echo);
