use std::fmt::{Display, Formatter, Result as FmtResult};

/// The discriminant of a [`Value`].
///
/// `#[repr(u8)]` and stable: these numbers are the tag the C ABI puts on the wire, so a variant's
/// value may never change and new variants may only be appended.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// [`Value::Bool`]
    Bool = 0,
    /// [`Value::Int`]
    Int = 1,
    /// [`Value::Float`]
    Float = 2,
    /// [`Value::Str`]
    Str = 3,
    /// [`Value::List`]
    List = 4,
    /// [`Value::Map`]
    Map = 5,
}

impl Kind {
    /// The lowercase name of this kind, as it appears in errors and logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int => "int",
            Self::Float => "float",
            Self::Str => "string",
            Self::List => "list",
            Self::Map => "map",
        }
    }

    /// Recover a kind from its ABI tag, or `None` if the tag is one this build does not know.
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Bool),
            1 => Some(Self::Int),
            2 => Some(Self::Float),
            3 => Some(Self::Str),
            4 => Some(Self::List),
            5 => Some(Self::Map),
            _ => None,
        }
    }
}

impl Display for Kind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str(self.as_str())
    }
}

/// A dynamically typed value carried through a hook.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer.
    Int(i64),
    /// A 64-bit float.
    Float(f64),
    /// A UTF-8 string.
    Str(String),
    /// An ordered list of values.
    List(Vec<Value>),
    /// An ordered string-keyed map of values.
    Map(Map),
}

impl Value {
    /// An empty map. The usual starting point for a hook payload.
    pub fn map() -> Self {
        Self::Map(Map::new())
    }

    /// An empty list.
    pub fn list() -> Self {
        Self::List(Vec::new())
    }

    /// Which variant this is.
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Bool(_) => Kind::Bool,
            Self::Int(_) => Kind::Int,
            Self::Float(_) => Kind::Float,
            Self::Str(_) => Kind::Str,
            Self::List(_) => Kind::List,
            Self::Map(_) => Kind::Map,
        }
    }

    /// The boolean inside, or `None` if this is not a [`Value::Bool`].
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The integer inside, or `None` if this is not a [`Value::Int`].
    pub const fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The float inside, or `None` if this is not a [`Value::Float`].
    pub const fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(inner) => Some(*inner),
            _ => None,
        }
    }

    /// The string inside, or `None` if this is not a [`Value::Str`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(inner) => Some(inner),
            _ => None,
        }
    }

    /// The list inside, or `None` if this is not a [`Value::List`].
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Self::List(inner) => Some(inner),
            _ => None,
        }
    }

    /// The list inside, mutably, or `None` if this is not a [`Value::List`].
    pub fn as_list_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Self::List(inner) => Some(inner),
            _ => None,
        }
    }

    /// The map inside, or `None` if this is not a [`Value::Map`].
    pub fn as_map(&self) -> Option<&Map> {
        match self {
            Self::Map(inner) => Some(inner),
            _ => None,
        }
    }

    /// The map inside, mutably, or `None` if this is not a [`Value::Map`].
    pub fn as_map_mut(&mut self) -> Option<&mut Map> {
        match self {
            Self::Map(inner) => Some(inner),
            _ => None,
        }
    }

    /// Walk a dotted path (`"server.listen.port"`), descending through maps and into lists by
    /// numeric segment. Returns `None` as soon as a segment does not resolve.
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let mut current = self;
        for segment in path.split('.') {
            match current {
                Self::Map(map) => match map.get(segment) {
                    Some(next) => current = next,
                    None => return None,
                },
                Self::List(list) => match segment.parse::<usize>() {
                    Ok(index) => match list.get(index) {
                        Some(next) => current = next,
                        None => return None,
                    },
                    Err(_) => return None,
                },
                _ => return None,
            }
        }
        Some(current)
    }

    /// [`get_path`](Self::get_path), mutably.
    pub fn get_path_mut(&mut self, path: &str) -> Option<&mut Value> {
        let mut current = self;
        for segment in path.split('.') {
            match current {
                Self::Map(map) => match map.get_mut(segment) {
                    Some(next) => current = next,
                    None => return None,
                },
                Self::List(list) => match segment.parse::<usize>() {
                    Ok(index) => match list.get_mut(index) {
                        Some(next) => current = next,
                        None => return None,
                    },
                    Err(_) => return None,
                },
                _ => return None,
            }
        }
        Some(current)
    }
}

impl Display for Value {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::Bool(inner) => write!(formatter, "{inner}"),
            Self::Int(inner) => write!(formatter, "{inner}"),
            Self::Float(inner) => write!(formatter, "{inner}"),
            Self::Str(inner) => write!(formatter, "{inner:?}"),
            Self::List(inner) => {
                formatter.write_str("[")?;
                for (index, item) in inner.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{item}")?;
                }
                formatter.write_str("]")
            }
            Self::Map(inner) => write!(formatter, "{inner}"),
        }
    }
}

impl From<bool> for Value {
    fn from(inner: bool) -> Self {
        Self::Bool(inner)
    }
}

impl From<i64> for Value {
    fn from(inner: i64) -> Self {
        Self::Int(inner)
    }
}

impl From<f64> for Value {
    fn from(inner: f64) -> Self {
        Self::Float(inner)
    }
}

impl From<String> for Value {
    fn from(inner: String) -> Self {
        Self::Str(inner)
    }
}

impl From<&str> for Value {
    fn from(inner: &str) -> Self {
        Self::Str(inner.to_string())
    }
}

impl From<Vec<Value>> for Value {
    fn from(inner: Vec<Value>) -> Self {
        Self::List(inner)
    }
}

impl From<Map> for Value {
    fn from(inner: Map) -> Self {
        Self::Map(inner)
    }
}

/// An ordered, string-keyed map of [`Value`]s.
///
/// Backed by a `Vec` rather than a hash map: hook payloads and plugin configuration are small,
/// insertion order is what people expect to see in logs and round-trips, and a flat vector is the
/// shape the C ABI can hand out by index without allocating an iterator.
///
/// Keys are unique — [`insert`](Self::insert) replaces an existing key in place, keeping its
/// original position.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    /// An empty map.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// An empty map with room for `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// How many entries the map holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value stored under `key`, if any.
    pub fn get(&self, key: &str) -> Option<&Value> {
        for (name, value) in &self.entries {
            if name == key {
                return Some(value);
            }
        }
        None
    }

    /// The value stored under `key`, mutably, if any.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        for (name, value) in &mut self.entries {
            if name == key {
                return Some(value);
            }
        }
        None
    }

    /// Whether `key` is present.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Store `value` under `key`, returning the value it replaced.
    ///
    /// Replacing keeps the key in its original position; a new key is appended.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        let key = key.into();
        for (name, slot) in &mut self.entries {
            if *name == key {
                return Some(std::mem::replace(slot, value));
            }
        }
        self.entries.push((key, value));
        None
    }

    /// Remove `key`, returning the value it held.
    ///
    /// Later entries shift down, so the order of everything else is preserved.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        for (index, (name, _)) in self.entries.iter().enumerate() {
            if name == key {
                let (_, value) = self.entries.remove(index);
                return Some(value);
            }
        }
        None
    }

    /// The key and value at `index`, in insertion order.
    ///
    /// This is what the C ABI iterates with, since it cannot hold a Rust iterator across the
    /// boundary.
    pub fn entry_at(&self, index: usize) -> Option<(&str, &Value)> {
        match self.entries.get(index) {
            Some((name, value)) => Some((name, value)),
            None => None,
        }
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Iterate the entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }

    /// Iterate the entries in insertion order, with mutable values.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut Value)> {
        self.entries
            .iter_mut()
            .map(|(name, value)| (name.as_str(), value))
    }

    /// Iterate the keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }
}

impl Display for Map {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
        formatter.write_str("{")?;
        for (index, (name, value)) in self.entries.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{name:?}: {value}")?;
        }
        formatter.write_str("}")
    }
}

impl<'a> IntoIterator for &'a Map {
    type Item = (&'a str, &'a Value);
    type IntoIter = Box<dyn Iterator<Item = (&'a str, &'a Value)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<K: Into<String>> FromIterator<(K, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (K, Value)>>(iterator: I) -> Self {
        let mut map = Self::new();
        for (key, value) in iterator {
            map.insert(key, value);
        }
        map
    }
}
