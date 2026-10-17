#![forbid(unsafe_code)]
//! Serves the Pagura session protocol over standard input and output (ADR 0005).

use pagura_host::{Session, SessionEnd};
use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    match Session::default().serve(&mut io::stdin().lock(), &mut io::stdout().lock()) {
        Ok(SessionEnd::Eof) => ExitCode::SUCCESS,
        Ok(SessionEnd::ProtocolError) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("pagura-host: transport failure: {error}");
            ExitCode::FAILURE
        }
    }
}
