#![forbid(unsafe_code)]
//! Exploratory ECDSA verification timings: `PublicKey::verify_der` over the
//! valid signatures of the `crypto/ecdsa-signatures` fixture, per curve, with
//! every key parsed beforehand.
//!
//! ```text
//! cargo run --release -p pagura-crypto --example ecdsa_profile -- [PASSES]
//! ```
//!
//! Timings are hypotheses until the performance policy in
//! `docs/ARCHITECTURE.md` is met.

use pagura_crypto::Verification;
use pagura_crypto::ecdsa::{Curve, PublicKey};
use pagura_testkit::{fixture_text, unhex};
use std::hint::black_box;
use std::time::Instant;

type Failure = Box<dyn std::error::Error>;

fn main() -> Result<(), Failure> {
    let passes: usize = std::env::args()
        .nth(1)
        .map_or(Ok(20), |value| value.parse())?;
    let vectors = fixture_text("crypto", "ecdsa-signatures.vectors");
    for (label, curve) in [
        ("P256", Curve::P256),
        ("P384", Curve::P384),
        ("P521", Curve::P521),
    ] {
        let mut cases = Vec::new();
        for line in vectors.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            if let [_, _, name, digest, key, signature, "1"] = fields.as_slice()
                && *name == label
            {
                cases.push((
                    PublicKey::from_sec1(curve, &unhex(key))?,
                    unhex(digest),
                    unhex(signature),
                ));
            }
        }
        let start = Instant::now();
        for _ in 0..passes {
            for (key, digest, signature) in &cases {
                if key.verify_der(black_box(digest), black_box(signature))? != Verification::Valid {
                    return Err(format!("{label}: a valid signature does not verify").into());
                }
            }
        }
        let count = u32::try_from(passes * cases.len())?;
        let micros = start.elapsed().as_secs_f64() * 1e6 / f64::from(count);
        println!("{label}: verify_der {micros:.1} us ({count} verifications)");
    }
    Ok(())
}
