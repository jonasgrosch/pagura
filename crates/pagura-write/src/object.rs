//! Objects handed to the builder.

use pagura_core::Value;

/// An object to write: a value, plus data if it is a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct NewObject {
    /// The value; a stream's value must be its dictionary.
    pub value: Value,
    /// Exact stored stream bytes. If `/Filter` is supplied, the caller must
    /// encode these bytes accordingly (7.3.8.2); this serializer does not encode.
    /// `/Length` is written automatically from the stored byte count.
    pub stream: Option<Vec<u8>>,
}

impl NewObject {
    /// A non-stream object.
    pub fn value(value: Value) -> Self {
        Self {
            value,
            stream: None,
        }
    }

    /// A stream object whose data is already encoded if its dictionary has filters.
    pub fn stream(dictionary: pagura_core::Dictionary, data: Vec<u8>) -> Self {
        Self {
            value: Value::Dictionary(dictionary),
            stream: Some(data),
        }
    }
}
