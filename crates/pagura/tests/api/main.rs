#![forbid(unsafe_code)]
//! The facade's public API, across the workspace crates, in one test crate
//! so that the modules share `support` and one link of the engine.

mod annotations;
mod document;
mod editing;
mod facade;
mod overprint;
mod provider;
mod render;
mod signatures;
mod streaming;
mod support;
mod tags;
mod text;
mod write;
