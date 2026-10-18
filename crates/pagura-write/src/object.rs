//! Objects handed to the builder.

use pagura_core::{Dictionary, Value};

/// An indirect object to write (7.3.10): a value, or a stream.
///
/// The enum is exhaustive: a stream is the one kind of object that can only
/// be indirect (7.3.8.1), and every other object is a [`Value`], so these two
/// cases are all there are.
#[derive(Debug, Clone, PartialEq)]
pub enum NewObject {
    /// Any object other than a stream.
    Value(Value),
    /// A stream (7.3.8).
    Stream {
        /// The stream dictionary. The writer writes `/Length` from `data`,
        /// in place of any `/Length` given here.
        dictionary: Dictionary,
        /// The stored bytes, written exactly. If `dictionary` declares
        /// `/Filter`, they must already be encoded accordingly (7.3.8.2):
        /// the writer does not encode.
        data: Vec<u8>,
    },
}

impl From<Value> for NewObject {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}
