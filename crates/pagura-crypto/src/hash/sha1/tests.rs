use super::*;
use crate::hash::{Sha224, sha224};
use crate::rsa::tests::{decode, vectors};

#[test]
fn nist_minimized_and_streaming() {
    for line in include_str!("nist-subset.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
    {
        let fields: Vec<_> = line.split('|').collect();
        let [algorithm, input, expected] = fields.as_slice() else {
            unreachable!()
        };
        let input = decode(input);
        let expected = decode(expected);
        for size in [1, 3, 55, 56, 63, 64, 65, 128] {
            if *algorithm == "SHA1" {
                let mut hash = Sha1::new();
                for chunk in input.chunks(size) {
                    hash.update(&[]);
                    hash.update(chunk);
                }
                assert_eq!(hash.clone().finalize().as_slice(), expected);
                assert_eq!(hash.finalize().as_slice(), expected);
                assert_eq!(sha1(&input).as_slice(), expected);
            } else {
                let mut hash = Sha224::new();
                for chunk in input.chunks(size) {
                    hash.update(&[]);
                    hash.update(chunk);
                }
                assert_eq!(hash.clone().finalize().as_slice(), expected);
                assert_eq!(hash.finalize().as_slice(), expected);
                assert_eq!(sha224(&input).as_slice(), expected);
            }
        }
    }
}

#[test]
fn cached_cavp_byte_suites_including_monte_carlo() {
    let data = vectors("hash");
    let mut counts = std::collections::BTreeMap::new();
    for line in data.lines() {
        if line.starts_with('#') {
            eprintln!("{line}");
            continue;
        }
        let fields: Vec<_> = line.split('|').collect();
        let [kind, algorithm, input, expected] = fields.as_slice() else {
            unreachable!()
        };
        let input = decode(input);
        let actual = if *kind == "monte" {
            let mut a = input.clone();
            let mut b = input.clone();
            let mut c = input.clone();
            for _ in 0..1000 {
                let joined = [a.as_slice(), b.as_slice(), c.as_slice()].concat();
                let next = if *algorithm == "SHA1" {
                    sha1(&joined).to_vec()
                } else {
                    sha224(&joined).to_vec()
                };
                a = b;
                b = c;
                c = next;
            }
            c
        } else if *algorithm == "SHA1" {
            let mut streamed = Sha1::new();
            for chunk in input.chunks(33) {
                streamed.update(chunk);
            }
            let actual = sha1(&input).to_vec();
            assert_eq!(streamed.finalize().as_slice(), actual);
            actual
        } else {
            let mut streamed = Sha224::new();
            for chunk in input.chunks(33) {
                streamed.update(chunk);
            }
            let actual = sha224(&input).to_vec();
            assert_eq!(streamed.finalize().as_slice(), actual);
            actual
        };
        assert_eq!(actual, decode(expected), "{kind} {algorithm}");
        *counts.entry((*kind, *algorithm)).or_insert(0) += 1;
    }
    for ((kind, algorithm), count) in counts {
        eprintln!("CAVP {algorithm} {kind}: {count} passed");
    }
}
