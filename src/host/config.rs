//! Checking a plugin's configuration against the spec it declared.
//!
//! A [`ConfigSpec`](crate::plugin::ConfigSpec) is data, not code — a plain [`Value`] tree the
//! plugin hands over the ABI. That is what lets a plugin written in any language describe what it
//! wants without linking a schema library. The **host** decides how to check it, and with this
//! feature it hands the tree to `tanzim-validate`, which both validates and coerces.
//!
//! Values make a round trip through `tanzim_value::Value` to do it. Coercion is the reason the
//! validated configuration is returned rather than merely checked: a spec that says "integer" will
//! turn the string `"8080"` into `8080` before the plugin ever sees it.

use crate::value::{Map, Value};
use tanzim_validate::{
    LocatedValue, Location, Map as TanzimMap, Value as TanzimValue, build_value,
};

/// The synthetic origin stamped on values that came across the plugin boundary.
///
/// They have no file, line or column: the plugin handed us a tree, not a document. Naming the
/// origin `plugin` at least makes a validation error say where it really came from.
fn origin() -> Location {
    Location::at("plugin", "", None, None, None)
}

/// Translate a plugx value into the tanzim value the validator works on.
fn to_tanzim(value: &Value) -> TanzimValue {
    match value {
        Value::Bool(inner) => TanzimValue::Bool(*inner),
        // tanzim counts in `isize`. On a 32-bit target a plugin could hand over an integer that
        // does not fit; turning it into a string rather than truncating means an integer validator
        // rejects it with a message, instead of the plugin silently seeing a different number.
        Value::Int(inner) => match isize::try_from(*inner) {
            Ok(inner) => TanzimValue::Int(inner),
            Err(_) => TanzimValue::String(inner.to_string()),
        },
        Value::Float(inner) => TanzimValue::Float(*inner),
        Value::Str(inner) => TanzimValue::String(inner.clone()),
        Value::List(item_list) => {
            let mut converted_list = Vec::with_capacity(item_list.len());
            for item in item_list {
                converted_list.push(LocatedValue::new(to_tanzim(item), origin()));
            }
            TanzimValue::List(converted_list)
        }
        Value::Map(entries) => {
            let mut converted = TanzimMap::new();
            for (key, item) in entries.iter() {
                converted.insert(
                    key.to_string(),
                    LocatedValue::new(to_tanzim(item), origin()),
                );
            }
            TanzimValue::Map(converted)
        }
    }
}

/// Translate a validated tanzim value back into a plugx value.
///
/// `Null` has no plugx counterpart — the six variants are what crosses the ABI — so a null is
/// dropped from a map and reported as `None` anywhere else.
fn from_tanzim(value: &TanzimValue) -> Option<Value> {
    match value {
        TanzimValue::Bool(inner) => Some(Value::Bool(*inner)),
        TanzimValue::Int(inner) => match i64::try_from(*inner) {
            Ok(inner) => Some(Value::Int(inner)),
            Err(_) => None,
        },
        TanzimValue::Float(inner) => Some(Value::Float(*inner)),
        TanzimValue::String(inner) => Some(Value::Str(inner.clone())),
        TanzimValue::List(item_list) => {
            let mut converted_list = Vec::with_capacity(item_list.len());
            for item in item_list {
                if let Some(item) = from_tanzim(item.value()) {
                    converted_list.push(item);
                }
            }
            Some(Value::List(converted_list))
        }
        TanzimValue::Map(entries) => {
            let mut converted = Map::with_capacity(entries.len());
            for (key, item) in entries.entries() {
                if let Some(item) = from_tanzim(item.value()) {
                    converted.insert(key.clone(), item);
                }
            }
            Some(Value::Map(converted))
        }
        TanzimValue::Null => None,
    }
}

/// Validate and coerce `config` against `spec`, returning the configuration the plugin should see.
///
/// The error is already rendered: `tanzim-validate` points at the offending value, so there is
/// nothing useful for the caller to add.
pub fn validate(spec: &Value, config: Value) -> Result<Value, Box<str>> {
    let validator = match build_value(&to_tanzim(spec)) {
        Ok(validator) => validator,
        Err(error) => return Err(format!("invalid config spec: {error}").into_boxed_str()),
    };
    let mut located = LocatedValue::new(to_tanzim(&config), origin());
    if let Err(error) = tanzim_validate::validate(validator.as_ref(), &mut located) {
        return Err(format!("{error}").into_boxed_str());
    }
    match from_tanzim(located.value()) {
        Some(config) => Ok(config),
        None => Err(Box::from("configuration validated to nothing")),
    }
}
