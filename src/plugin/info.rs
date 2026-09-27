use crate::value::{Map, Value};
use std::fmt::{Display, Formatter, Result as FmtResult};

/// A plugin's own version. Independent of the ABI version it was built against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Version {
    /// Incompatible change.
    pub major: u32,
    /// Backwards-compatible addition.
    pub minor: u32,
    /// A fix.
    pub patch: u32,
}

impl Version {
    /// Build a version.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parse `"1.2.3"`. Returns `None` on anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let mut part_list = text.split('.');
        let major = part_list.next()?.parse().ok()?;
        let minor = part_list.next()?.parse().ok()?;
        let patch = part_list.next()?.parse().ok()?;
        if part_list.next().is_some() {
            return None;
        }
        Some(Self::new(major, minor, patch))
    }
}

impl Display for Version {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Another plugin this one needs, and the versions of it that will do.
///
/// A host resolves dependencies before starting anything, so a plugin can assume everything it
/// declared here is already started by the time its own `start` runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    /// The name of the plugin required.
    pub name: String,
    /// The oldest acceptable version.
    pub minimum: Version,
    /// The first version that is *not* acceptable, or `None` for no upper bound.
    pub before: Option<Version>,
    /// Whether the host may start without it. An optional dependency that is present is still
    /// ordered before this plugin.
    pub optional: bool,
}

impl Dependency {
    /// Require `name` at `minimum` or later, with no upper bound.
    pub fn new(name: impl Into<String>, minimum: Version) -> Self {
        Self {
            name: name.into(),
            minimum,
            before: None,
            optional: false,
        }
    }

    /// Whether `version` satisfies this dependency.
    pub fn accepts(&self, version: Version) -> bool {
        if version < self.minimum {
            return false;
        }
        match self.before {
            Some(before) => version < before,
            None => true,
        }
    }
}

/// The shape a plugin's configuration must have, expressed as data rather than as code.
///
/// A spec is a plain [`Value`] tree, which is what lets it cross the ABI without either side
/// depending on a schema library: the plugin describes what it wants, and the **host** decides how
/// to check it. A host with the right feature enabled hands the tree to a real schema validator;
/// one without it can still show the spec to a user.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigSpec {
    schema: Option<Value>,
}

impl ConfigSpec {
    /// A plugin that takes no configuration.
    pub const fn none() -> Self {
        Self { schema: None }
    }

    /// A spec described by `schema`.
    pub const fn new(schema: Value) -> Self {
        Self {
            schema: Some(schema),
        }
    }

    /// The schema tree, or `None` if this plugin takes no configuration.
    pub const fn schema(&self) -> Option<&Value> {
        self.schema.as_ref()
    }

    /// Take the schema tree out.
    pub fn into_schema(self) -> Option<Value> {
        self.schema
    }
}

/// Everything a plugin reports about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    /// The plugin's identity: the namespace its registrations are filed under and how other plugins address
    /// its functions.
    ///
    /// **Filled in by the runtime**, not by the plugin author. A plugin's name is its filename,
    /// parsed by the runtime that claimed it, and it is the runtime that leaks it and stamps it
    /// here — which is how a host learns what to call a plugin it has just built. Whatever a
    /// plugin writes here is replaced.
    pub name: &'static str,
    /// The plugin's own version.
    pub version: Version,
    /// One line saying what the plugin does.
    pub description: String,
    /// The shape of the configuration it expects.
    pub config_spec: ConfigSpec,
    /// The plugins it needs, and in what versions.
    pub dependency_list: Vec<Dependency>,
}

impl Info {
    /// The minimum: a version and a description, no configuration and no dependencies.
    pub fn new(version: Version, description: impl Into<String>) -> Self {
        Self {
            name: "",
            version,
            description: description.into(),
            config_spec: ConfigSpec::none(),
            dependency_list: Vec::new(),
        }
    }

    /// Declare the configuration shape.
    pub fn with_config_spec(mut self, config_spec: ConfigSpec) -> Self {
        self.config_spec = config_spec;
        self
    }

    /// Declare a dependency.
    pub fn with_dependency(mut self, dependency: Dependency) -> Self {
        self.dependency_list.push(dependency);
        self
    }

    /// Render this info as a [`Value`] tree, which is the form it crosses the ABI in.
    pub fn to_value(&self) -> Value {
        let mut root = Map::with_capacity(4);
        root.insert("version", Value::Str(self.version.to_string()));
        root.insert("description", Value::Str(self.description.clone()));
        if let Some(schema) = self.config_spec.schema() {
            root.insert("config_spec", schema.clone());
        }
        let mut dependency_list = Vec::with_capacity(self.dependency_list.len());
        for dependency in &self.dependency_list {
            let mut entry = Map::with_capacity(4);
            entry.insert("name", Value::Str(dependency.name.clone()));
            entry.insert("minimum", Value::Str(dependency.minimum.to_string()));
            if let Some(before) = dependency.before {
                entry.insert("before", Value::Str(before.to_string()));
            }
            if dependency.optional {
                entry.insert("optional", Value::Bool(true));
            }
            dependency_list.push(Value::Map(entry));
        }
        if !dependency_list.is_empty() {
            root.insert("dependencies", Value::List(dependency_list));
        }
        Value::Map(root)
    }

    /// Read back what [`to_value`](Self::to_value) wrote.
    ///
    /// Returns `None` if the tree is not a map, or its `version` is not parseable.
    pub fn from_value(value: &Value) -> Option<Self> {
        let root = value.as_map()?;
        let version = Version::parse(root.get("version")?.as_str()?)?;
        let description = match root.get("description") {
            Some(Value::Str(text)) => text.clone(),
            _ => String::new(),
        };
        let config_spec = match root.get("config_spec") {
            Some(schema) => ConfigSpec::new(schema.clone()),
            None => ConfigSpec::none(),
        };
        let mut dependency_list = Vec::new();
        if let Some(Value::List(item_list)) = root.get("dependencies") {
            for item in item_list {
                let entry = item.as_map()?;
                let name = entry.get("name")?.as_str()?.to_string();
                let minimum = Version::parse(entry.get("minimum")?.as_str()?)?;
                let before = match entry.get("before") {
                    Some(Value::Str(text)) => Some(Version::parse(text)?),
                    _ => None,
                };
                let optional = match entry.get("optional") {
                    Some(Value::Bool(flag)) => *flag,
                    _ => false,
                };
                dependency_list.push(Dependency {
                    name,
                    minimum,
                    before,
                    optional,
                });
            }
        }
        Some(Self {
            name: "",
            version,
            description,
            config_spec,
            dependency_list,
        })
    }
}
