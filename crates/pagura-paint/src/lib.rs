#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! The renderer (ADR 0034): [`prepare`] turns a recorded scene and a
//! [`RenderRequest`] into a [`PreparedScene`], validating every input and
//! reserving every budget in source order. A render takes its storage from
//! a caller's [`Scratch`] ([`PreparedScene::with_scratch`]), then
//! [`Rendering::render`] executes bands of the global device grid without
//! failing, composites through span kernels into per-buffer storage formats
//! and quantizes each output pixel once. Every pixel is within one level of
//! its exact result (ADR 0004).

mod blend;
mod execute;
mod format;
mod kernel;
mod plan;
mod prepare;
mod process;
mod request;
mod source;
#[cfg(test)]
mod testing;

pub use execute::{Rendering, Scratch};

#[cfg(feature = "gpu-demo")]
pub use prepare::gpu;
pub use prepare::{Charges, Completion, PreparedScene, prepare};
pub use request::{OverprintMode, RenderRequest};
