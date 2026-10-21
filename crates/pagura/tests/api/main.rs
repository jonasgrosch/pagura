#![forbid(unsafe_code)]
//! The facade's public API, across the workspace crates, in one test crate
//! so that the modules share `support` and one link of the engine.

mod document;
mod editing;
mod overprint;
mod provider;
mod render;
mod signatures;
mod support;
mod text;
mod write;
