#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! A slow, obviously correct reference renderer for differential tests
//! (ADR 0034): exact covered areas by polygon clipping in f64, and the
//! imaging model of clause 11 evaluated per pixel in f64, over a recorded
//! `pagura_interpret::Scene`. Test only; never a product dependency.
