#![forbid(unsafe_code)]
//! Exploratory timings of the standard security handler on the generated
//! crypto fixtures: revision 5 and 6 password authentication (Algorithm 2.B
//! is dominated by AES-128-CBC and SHA-2) and AESV3 decryption of a
//! string-sized object and a 1 MiB stream.
//!
//! ```text
//! cargo run --release -p pagura-crypto --example security_profile -- [ROUNDS]
//! ```
//!
//! Timings are hypotheses until the performance policy in
//! `docs/ARCHITECTURE.md` is met.

use pagura_core::ObjectId;
use pagura_crypto::standard::{CryptMethod, FileKey, SecurityParams, authenticate};
use pagura_testkit::{fixture, fixture_text, unhex};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

type Failure = Box<dyn std::error::Error>;

const STREAM_BYTES: usize = 1 << 20;

fn params(name: &str) -> Result<SecurityParams, Failure> {
    let text = fixture_text("crypto", &format!("{name}.params"));
    let fields: BTreeMap<_, _> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    let field = |key: &str| fields.get(key).copied().ok_or(format!("{name}: no {key}"));
    Ok(SecurityParams {
        version: field("version")?.parse()?,
        revision: field("revision")?.parse()?,
        key_length: field("key_length")?.parse()?,
        owner_hash: unhex(field("owner_hash")?),
        user_hash: unhex(field("user_hash")?),
        owner_encrypted_key: Some(unhex(field("owner_encrypted_key")?)),
        user_encrypted_key: Some(unhex(field("user_encrypted_key")?)),
        permissions: field("permissions")?.parse()?,
        encrypted_permissions: Some(unhex(field("encrypted_permissions")?)),
        encrypt_metadata: field("encrypt_metadata")? == "1",
        document_id: unhex(field("document_id")?),
    })
}

/// IV plus `STREAM_BYTES` of ciphertext whose last block decrypts to valid
/// padding, found by adjusting one byte of the block before it.
fn stream(key: &FileKey, object: ObjectId) -> Result<Vec<u8>, Failure> {
    let mut data: Vec<u8> = (0..16 + STREAM_BYTES)
        .map(|index| u8::try_from(index * 31 % 251).unwrap_or(0))
        .collect();
    // The last 48 bytes: an IV and the two final blocks.
    let (tail, adjusted) = (data.len() - 48, data.len() - 17);
    for value in 0..=u8::MAX {
        data[adjusted] = value;
        if key
            .decrypt(CryptMethod::AesV3, object, &data[tail..])
            .is_ok()
        {
            return Ok(data);
        }
    }
    Err("no padding byte decrypts".into())
}

fn main() -> Result<(), Failure> {
    let rounds: usize = std::env::args()
        .nth(1)
        .map_or(Ok(20), |value| value.parse())?;
    let object = ObjectId::new(1, 0);
    for name in ["r6-aesv3-password", "r5-aesv3-password"] {
        let params = params(name)?;
        let password = fixture("crypto", &format!("{name}.user"));
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(authenticate(black_box(&params), black_box(&password))?);
        }
        let micros = start.elapsed().as_secs_f64() * 1e6 / f64::from(u32::try_from(rounds)?);
        println!("{name}: authenticate {micros:.1} us ({rounds} rounds)");
        let key = authenticate(&params, &password)?;
        let string = fixture("crypto", &format!("{name}.cipher"));
        let count = 100_000;
        let start = Instant::now();
        for _ in 0..count {
            black_box(key.decrypt(CryptMethod::AesV3, object, black_box(&string))?);
        }
        let nanos = start.elapsed().as_secs_f64() * 1e9 / f64::from(count);
        println!("{name}: decrypt {}-byte string {nanos:.0} ns", string.len());
        let data = stream(&key, object)?;
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(key.decrypt(CryptMethod::AesV3, object, black_box(&data))?);
        }
        let seconds = start.elapsed().as_secs_f64() / f64::from(u32::try_from(rounds)?);
        println!("{name}: decrypt 1 MiB stream {:.1} MiB/s", 1.0 / seconds);
    }
    Ok(())
}
