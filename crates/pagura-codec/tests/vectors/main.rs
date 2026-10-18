#![forbid(unsafe_code)]
//! Generated codec vectors (`corpus/generated/codec`): decoding against
//! independent expected bytes, and encoder round trips.

mod decode;
mod encode;

use pagura_testkit::fixture;
use std::collections::BTreeMap;

/// The `key=value` lines of a vector's `.params` file.
fn options(name: &str) -> BTreeMap<String, String> {
    String::from_utf8(fixture("codec", &format!("{name}.params")))
        .unwrap()
        .lines()
        .map(|line| {
            let (key, value) = line.split_once('=').unwrap();
            (key.to_owned(), value.to_owned())
        })
        .collect()
}
