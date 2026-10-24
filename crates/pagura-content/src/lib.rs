#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! PDF content streams (7.8.2, Annex A): a tokenizer over the concatenated
//! content of a page or form, typed operations with validated operands,
//! compatibility sections, inline images, and source positions for every
//! operation (ADR 0034).
//!
//! [`Content`] holds a page's or form's streams as one token sequence;
//! [`Reader`] yields its [`Operation`]s, one at a time: strings, `TJ` arrays
//! and colour components borrow from the reader until the next operation,
//! so reading allocates nothing per operation. The interpreter consumes
//! them; editing and redaction will filter them. This crate knows nothing
//! about graphics state, resources or rendering.

mod content;
mod number;
mod op;
mod operator;
mod reader;
mod token;

use operator::Operator;

pub use content::Content;
pub use op::{ColorOperands, Dash, Op, Painting, Properties, Rule, TextArray, TextItem, TextItems};
pub use reader::{Operation, Reader};
