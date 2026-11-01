#![forbid(unsafe_code)]
//! The facade's public API, across the workspace crates, in one test crate
//! so that the modules share `support` and one link of the engine.

mod annotation_compat;
mod annotation_empty_widgets;
mod annotations;
mod calculator;
mod document;
mod editing;
mod facade;
mod font_descriptor;
mod overprint;
mod provider;
mod render;
mod selected_fonts;
mod signatures;
mod streaming;
mod support;
mod tags;
mod text;
mod write;
