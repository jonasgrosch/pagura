#![forbid(unsafe_code)]
//! Exploratory RSA PKCS #1 v1.5 verification timings with prepared keys.
//!
//! The 2048-, 3072- and 4096-bit Wycheproof keys and signatures come from
//! the private reference cache; `python3 tools/signature_vectors.py` exports
//! them to `$PAGURA_REFERENCES_DIR/signatures/export/crypto/rsa-timing.txt`:
//!
//! ```text
//! cargo run --release -p pagura-crypto --example rsa_profile -- PATH
//! ```
//!
//! Timings are hypotheses until the performance policy in
//! `docs/ARCHITECTURE.md` is met.

use pagura_crypto::{HashAlgorithm, RsaLimits, RsaPublicKey, Verification};
use pagura_testkit::unhex;
use std::hint::black_box;
use std::time::Instant;

const WARM_UP: usize = 100;
const SAMPLES: usize = 2000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("expected the rsa-timing.txt file that tools/signature_vectors.py exports")?;
    for line in std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.starts_with('#'))
    {
        let fields: Vec<_> = line.split('|').collect();
        let [bits, n, e, digest, signature] = fields.as_slice() else {
            return Err(format!("malformed timing vector: {line}").into());
        };
        let key = RsaPublicKey::new(&unhex(n), &unhex(e), RsaLimits::default())?;
        let digest = unhex(digest);
        let signature = unhex(signature);
        let verify = || -> Result<(), Box<dyn std::error::Error>> {
            let digest = black_box(digest.as_slice());
            match key.verify_pkcs1v15(HashAlgorithm::Sha256, digest, black_box(&signature))? {
                Verification::Valid => Ok(()),
                Verification::Invalid => {
                    Err(format!("RSA-{bits} signature does not verify").into())
                }
            }
        };
        for _ in 0..WARM_UP {
            verify()?;
        }
        let start = Instant::now();
        for _ in 0..SAMPLES {
            verify()?;
        }
        let micros = start.elapsed().as_secs_f64() * 1e6 / f64::from(u32::try_from(SAMPLES)?);
        println!("RSA-{bits}: {micros:.3} us/verification ({SAMPLES} operations, prepared key)");
    }
    Ok(())
}
