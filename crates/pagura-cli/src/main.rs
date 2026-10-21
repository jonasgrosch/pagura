#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! The `pagura` command-line tool: inspection, text extraction, PDF-to-image
//! conversion, corpus scanning and internal profiling, built on the `pagura`
//! facade alone (ADR 0030).
//!
//! [`args::parse`] turns the arguments into a typed [`args::Command`];
//! [`commands::run`] executes it and writes to any [`std::io::Write`], so
//! both are unit-tested without a process. Exit status: 0 on success, 1
//! when the command fails, 2 for a usage error.

mod args;
mod commands;
mod failure;
mod input;
mod json;
mod microbench;
mod password;
mod scan;

use std::io::Write;
use std::process::ExitCode;

/// The exit status of a usage error, as for most command-line tools.
const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let command = match args::parse(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("pagura: {error}\n{}", args::USAGE);
            return ExitCode::from(USAGE_ERROR);
        }
    };
    let mut out = std::io::stdout().lock();
    let result = commands::run(&command, &mut out).and_then(|()| Ok(out.flush()?));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("pagura: {failure}");
            ExitCode::FAILURE
        }
    }
}
