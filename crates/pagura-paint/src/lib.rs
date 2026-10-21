#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! The renderer (ADR 0034): [`prepare`] turns a recorded scene and a
//! [`RenderRequest`] into a [`PreparedScene`], validating every input and
//! reserving every budget in source order; [`PreparedScene::render`] then
//! executes bands of the global device grid without failing, composites
//! through span kernels into per-buffer storage formats and quantizes each
//! output pixel once.
//!
//! Exact rendering (every pixel within one level of its exact result, ADR
//! 0004) is the default; [`FastParams`] switches the four named fast
//! parameters inside the same kernels.

mod format;
mod prepare;
mod request;
mod source;

pub use format::Format;
pub use prepare::{Completion, PreparedScene, prepare};
pub use request::{FastParams, OverprintMode, RenderRequest};
pub use source::{ImageSampler, PaintContext, ShadingSource, Source, SpanColors};
