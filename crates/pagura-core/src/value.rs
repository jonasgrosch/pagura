//! The PDF object model (ISO 32000-2 clause 7.3).

use crate::{Dictionary, Name};
use std::fmt;

/// An indirect object identity: object number plus generation.
///
/// Object number zero is reserved for the head of the free list and never
/// identifies a real object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId {
    /// Object number, at least 1 for real objects.
    pub number: u32,
    /// Generation number.
    pub generation: u16,
}

impl ObjectId {
    /// Creates an identity.
    pub const fn new(number: u32, generation: u16) -> Self {
        Self { number, generation }
    }
}

/// Formats as `number generation`, the way PDF writes it before `obj` or `R`.
impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.number, self.generation)
    }
}

/// A direct PDF object. Streams are indirect objects and live in the
/// document layer; their dictionary is a `Value::Dictionary`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The null object.
    Null,
    /// `true` or `false`.
    Boolean(bool),
    /// An integer object.
    Integer(i64),
    /// A real object; always finite when produced by the parser.
    Real(f64),
    /// A name object.
    Name(Name),
    /// A string object: bytes, whose text encoding depends on context.
    String(Vec<u8>),
    /// An array object.
    Array(Vec<Value>),
    /// A dictionary object.
    Dictionary(Dictionary),
    /// An indirect reference.
    Reference(ObjectId),
}

impl Value {
    /// Shorthand for a name value.
    pub fn name(bytes: &[u8]) -> Self {
        Self::Name(Name::new(bytes))
    }

    /// Whether this is the null object.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The boolean, if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// The integer, if this is an integer.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// The numeric value of an integer or finite real.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => Some(*value as f64),
            Self::Real(value) if value.is_finite() => Some(*value),
            _ => None,
        }
    }

    /// The name, if this is a name.
    pub fn as_name(&self) -> Option<&Name> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }

    /// The string bytes, if this is a string.
    pub fn as_string(&self) -> Option<&[u8]> {
        match self {
            Self::String(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    /// The dictionary, if this is a dictionary.
    pub fn as_dictionary(&self) -> Option<&Dictionary> {
        match self {
            Self::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }

    /// The identity, if this is an indirect reference.
    pub fn as_reference(&self) -> Option<ObjectId> {
        match self {
            Self::Reference(id) => Some(*id),
            _ => None,
        }
    }

    /// A short, stable description of the variant for error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Boolean(_) => "boolean",
            Self::Integer(_) => "integer",
            Self::Real(_) => "real",
            Self::Name(_) => "name",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Dictionary(_) => "dictionary",
            Self::Reference(_) => "reference",
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<Name> for Value {
    fn from(value: Name) -> Self {
        Self::Name(value)
    }
}

impl From<Dictionary> for Value {
    fn from(value: Dictionary) -> Self {
        Self::Dictionary(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Self::Array(value)
    }
}

impl From<ObjectId> for Value {
    fn from(value: ObjectId) -> Self {
        Self::Reference(value)
    }
}
