#![forbid(unsafe_code)]
//! Prints `web/protocol_constants.mjs`, generated from the wire tables:
//! `cargo run -p pagura-host --features codegen --example protocol_constants
//! > web/protocol_constants.mjs`.

fn main() {
    print!("{}", pagura_host::protocol::javascript_constants());
}
