use super::*;
use crate::der::Document;
use crate::validation::tests::fixture_text;
use crate::validation::{InvalidReason, PathState, SignatureInput, SignatureResult, Uncertainty};
use crate::x509::extensions::KeyUsage;
use pagura_core::SignatureLimits;
use pagura_testkit::unhex;
use std::collections::BTreeMap;

type Records = BTreeMap<Vec<Vec<u8>>, SignatureResult>;
fn key(input: SignatureInput<'_>) -> Vec<Vec<u8>> {
    vec![
        input.algorithm.element.encoded().to_vec(),
        input.message.to_vec(),
        [&[input.signature.unused_bits()], input.signature.as_bytes()].concat(),
        input.issuer_key.algorithm.as_bytes().to_vec(),
        input
            .issuer_key
            .parameters
            .map_or_else(Vec::new, |v| v.encoded().to_vec()),
        [
            &[input.issuer_key.key.unused_bits()],
            input.issuer_key.key.as_bytes(),
        ]
        .concat(),
    ]
}
fn time() -> Time<'static> {
    Time::from_utc(2020, 1, 1, 0, 0, 0).unwrap()
}
struct Data {
    certificates: BTreeMap<String, Vec<u8>>,
    crls: BTreeMap<String, Vec<u8>>,
    records: Records,
    cases: Vec<Vec<String>>,
}
impl Data {
    fn read(text: &str) -> Self {
        let mut data = Self {
            certificates: BTreeMap::new(),
            crls: BTreeMap::new(),
            records: BTreeMap::new(),
            cases: Vec::new(),
        };
        for line in text.lines() {
            let f = line.split('\t').collect::<Vec<_>>();
            match f[0] {
                "CERT" => {
                    data.certificates.insert(f[1].into(), unhex(f[2]));
                }
                "CRL" => {
                    data.crls.insert(f[1].into(), unhex(f[2]));
                }
                "SIGNATURE" => {
                    let k = f[1..7]
                        .iter()
                        .map(|s| if *s == "-" { Vec::new() } else { unhex(s) })
                        .collect();
                    data.records.insert(
                        k,
                        match f[7] {
                            "1" => SignatureResult::Verified,
                            "0" => SignatureResult::Invalid,
                            _ => SignatureResult::Unavailable,
                        },
                    );
                }
                "CASE" => data
                    .cases
                    .push(f[1..].iter().map(|s| (*s).into()).collect()),
                _ => panic!("unknown record {}", f[0]),
            }
        }
        data
    }
}

fn run_case(data: &Data, case: &[String], limits: &Limits) -> PathSearch {
    let options = ValidationOptions::new(time());
    run_with_options(
        data,
        case,
        limits,
        &options,
        case[4].split(',').count() - 1,
        0,
    )
}
fn run_with_options(
    data: &Data,
    case: &[String],
    limits: &Limits,
    options: &ValidationOptions<'_>,
    target: usize,
    anchor: usize,
) -> PathSearch {
    let documents = case[4]
        .split(',')
        .map(|n| Document::parse(&data.certificates[n], limits).unwrap())
        .collect::<Vec<_>>();
    let certificates = documents
        .iter()
        .map(|d| Certificate::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let crl_documents = case[5]
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|n| Document::parse(&data.crls[n], limits).unwrap())
        .collect::<Vec<_>>();
    let crls = crl_documents
        .iter()
        .map(|d| CertificateList::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let pool = certificates.iter().collect::<Vec<_>>();
    let crls = crls.iter().collect::<Vec<_>>();
    let anchor = TrustAnchor::from_certificate(&certificates[anchor]);
    let mut verifier = |input: SignatureInput<'_>, _: &Limits| {
        data.records
            .get(&key(input))
            .copied()
            .unwrap_or(SignatureResult::Unavailable)
    };
    build_paths_with_crls(
        target,
        &pool,
        &[anchor],
        &crls,
        options,
        &mut verifier,
        limits,
    )
}

#[test]
fn cached_pkits_offline_revocation() {
    let Some(text) = crate::validation::tests::exported("revocation.txt") else {
        return;
    };
    let data = Data::read(&text);
    let limits = Limits::default();
    let mut counts = BTreeMap::<u32, [usize; 5]>::new();
    let mut failures = Vec::new();
    for case in &data.cases {
        let result = run_case(&data, case, &limits);
        let good = result
            .paths
            .iter()
            .any(|p| p.report.state == PathState::Valid);
        let revoked = result.paths.iter().any(|p| {
            matches!(
                p.report.state,
                PathState::Invalid {
                    reason: InvalidReason::Revoked,
                    ..
                }
            )
        });
        let unknown = result.paths.iter().any(|p| {
            matches!(
                p.report.state,
                PathState::Indeterminate {
                    reason: Uncertainty::RevocationData(_),
                    ..
                }
            )
        });
        let matches = result.complete
            && good == (case[2] == "1")
            && match case[3].as_str() {
                "Good" => good,
                "Revoked" => revoked,
                "Unknown" => unknown,
                "BasicInvalid" => !good && !unknown,
                _ => panic!("unknown expectation"),
            };
        let count = counts.entry(case[1].parse().unwrap()).or_default();
        count[0] += 1;
        count[1] += usize::from(matches);
        count[2] += usize::from(good);
        count[3] += usize::from(!good && revoked);
        count[4] += usize::from(!good && !revoked && unknown);
        if !matches {
            failures.push((
                case[0].clone(),
                case[3].clone(),
                result.complete,
                result
                    .paths
                    .iter()
                    .map(|p| {
                        (
                            p.certificates.clone(),
                            p.report.basic.clone(),
                            p.report.state.clone(),
                            p.report.revocation.clone(),
                        )
                    })
                    .collect::<Vec<_>>(),
            ));
        }
    }
    for (section, counts) in counts {
        eprintln!(
            "PKITS 4.{section}: matches {}/{}, Good {}, Revoked {}, Unknown {}",
            counts[1], counts[0], counts[2], counts[3], counts[4]
        );
    }
    assert_eq!(data.cases.len(), 79);
    assert!(failures.is_empty(), "{failures:?}");
}

fn structural_signature(input: SignatureInput<'_>, _: &Limits) -> SignatureResult {
    match input.signature.as_bytes() {
        b"invalid" => SignatureResult::Invalid,
        b"unavailable" => SignatureResult::Unavailable,
        _ => SignatureResult::Verified,
    }
}
fn synthetic_crls() -> String {
    fixture_text("pki", "crls.txt")
}
fn assert_outcome(ident: &str, expected: &str, report: &PathReport) {
    let outcome = report.revocation().last().unwrap();
    let result = match (outcome, expected.split_once(':')) {
        (RevocationOutcome::Good, None) if expected == "Good" => {
            assert_eq!(report.state(), &PathState::Valid, "{ident}");
            true
        }
        (RevocationOutcome::Unknown(reason), Some(("Unknown", value))) => {
            assert!(
                matches!(report.state(), PathState::Indeterminate { .. }),
                "{ident}"
            );
            format!("{reason:?}") == value
        }
        (RevocationOutcome::Revoked { reason, time }, Some(("Revoked", value))) => {
            assert!(
                matches!(
                    report.state(),
                    PathState::Invalid {
                        reason: InvalidReason::Revoked,
                        ..
                    }
                ),
                "{ident}"
            );
            let expected_date = if ident == "future-revocation" {
                (2026, 1, 1)
            } else {
                (2024, 12, 1)
            };
            assert_eq!((time.year(), time.month(), time.day()), expected_date);
            format!("{reason:?}") == value
        }
        (_, None) if expected == "Signature" => matches!(
            report.state(),
            PathState::Indeterminate {
                reason: Uncertainty::Signature,
                ..
            }
        ),
        (_, None) if expected == "Limit" => matches!(
            report.state(),
            PathState::Indeterminate {
                reason: Uncertainty::Limit("pki.path-verifications"),
                ..
            }
        ),
        _ => false,
    };
    assert!(result, "{ident}: expected {expected}, {report:?}");
}
#[test]
fn independent_crl_scope_freshness_deltas_and_critical_extensions() {
    let mut count = 0;
    for line in synthetic_crls().lines() {
        let f = line.split('\t').collect::<Vec<_>>();
        let bytes = f[3..].iter().map(|s| unhex(s)).collect::<Vec<_>>();
        let parse_limits = Limits::default();
        let limits = if matches!(f[2], "Limit" | "InvalidBound") {
            Limits {
                signatures: SignatureLimits {
                    path_work: 1_500_000,
                    path_verifications: 3,
                    ..parse_limits.signatures
                },
                ..parse_limits
            }
        } else {
            parse_limits
        };
        let documents = bytes
            .iter()
            .map(|b| Document::parse(b, &parse_limits).unwrap())
            .collect::<Vec<_>>();
        let certificates = documents[..3]
            .iter()
            .map(|d| Certificate::parse(d.root()).unwrap())
            .collect::<Vec<_>>();
        let crls = documents[3..]
            .iter()
            .map(|d| CertificateList::parse(d.root()).unwrap())
            .collect::<Vec<_>>();
        let pool = certificates.iter().collect::<Vec<_>>();
        let crls = crls.iter().collect::<Vec<_>>();
        let anchor = TrustAnchor::from_certificate(&certificates[0]);
        let mut options = ValidationOptions::new(Time::from_utc(2025, 1, 1, 0, 0, 0).unwrap());
        let mut calls = 0;
        let mut verifier = |input: SignatureInput<'_>, limits: &Limits| {
            calls += 1;
            if f[2] == "Signature" && input.signature.as_bytes().starts_with(b"synthetic-") {
                SignatureResult::Unavailable
            } else {
                structural_signature(input, limits)
            }
        };
        if f[2] == "InvalidBound" {
            options.key_usage = KeyUsage::DIGITAL_SIGNATURE;
            let result =
                build_paths_with_crls(2, &pool, &[anchor], &crls, &options, &mut verifier, &limits);
            assert!(!result.complete && !result.exhaustive);
            assert_eq!(
                result.stopped,
                Some(Uncertainty::Limit("pki.path-verifications"))
            );
            assert!(matches!(
                result.paths[0].report.state(),
                PathState::Invalid {
                    reason: InvalidReason::KeyUsage,
                    ..
                }
            ));
            assert_eq!(calls, 3);
        } else if f[2] == "Untrusted" {
            let search =
                build_paths_with_crls(2, &pool, &[], &crls, &options, &mut verifier, &limits);
            assert!(search.paths.iter().all(|p| matches!(
                p.report.state(),
                PathState::Invalid {
                    reason: InvalidReason::Untrusted,
                    ..
                }
            )));
            assert!(
                search
                    .paths
                    .iter()
                    .all(|p| p.report.revocation().len() == p.certificates.len())
            );
            assert_eq!(calls, 0);
        } else {
            let report = validate_path_with_crls(
                &pool[1..],
                anchor,
                &pool[1..],
                &crls,
                &options,
                &mut verifier,
                &limits,
            );
            assert_eq!(report.revocation().len(), 2, "{}", f[1]);
            assert_outcome(f[1], f[2], &report);
            if f[1] == "memo-repeated-crls" {
                assert_eq!(calls, 4);
            }
            if f[2] == "Limit" {
                assert_eq!(calls, 3);
            }
        }
        count += 1;
    }
    assert_eq!(count, 64);
}

fn security_data() -> Data {
    Data::read(&fixture_text("pki", "path-security.txt"))
}
fn check_security_cases(accept: impl Fn(&[String]) -> bool, expected: usize) {
    let data = security_data();
    let limits = Limits::default();
    let mut count = 0;
    for case in data.cases.iter().filter(|case| accept(case)) {
        count += 1;
        let target = case[4].split(',').position(|name| name == case[1]).unwrap();
        for mode in [
            crate::validation::SearchMode::FirstAcceptable,
            crate::validation::SearchMode::All,
        ] {
            let mut options = ValidationOptions::new(Time::from_utc(2025, 1, 1, 0, 0, 0).unwrap());
            options.search = mode;
            let anchor = case[4].split(',').position(|name| name == case[3]).unwrap();
            let search = run_with_options(&data, case, &limits, &options, target, anchor);
            assert!(search.complete, "{}: {search:?}", case[0]);
            let valid = search
                .paths
                .iter()
                .any(|p| p.report.state() == &PathState::Valid);
            assert_eq!(valid, case[2] == "Valid", "{}: {search:?}", case[0]);
            if case[2] == "Signature" {
                assert!(
                    search.paths.iter().any(|p| {
                        p.report.basic()
                            == &PathState::Indeterminate {
                                certificate: Some(1),
                                reason: Uncertainty::Signature,
                            }
                            && p.report.state() == p.report.basic()
                            && p.report.revocation()
                                == [RevocationOutcome::Good, RevocationOutcome::Good]
                    }),
                    "{}: {search:?}",
                    case[0]
                );
            } else if case[2] == "NameForm" {
                assert!(
                    search.paths.iter().all(|p| matches!(
                        p.report.basic(),
                        PathState::Indeterminate {
                            reason: Uncertainty::NameForm,
                            ..
                        }
                    )),
                    "{}: {search:?}",
                    case[0]
                );
                assert!(!search.paths.is_empty());
            } else if case[2] == "Unknown" {
                assert!(
                    search.paths.iter().any(|p| {
                        p.report.basic() == &PathState::Valid
                            && matches!(p.report.state(), PathState::Indeterminate { .. })
                            && matches!(
                                p.report.revocation().last(),
                                Some(RevocationOutcome::Unknown(_))
                            )
                    }),
                    "{}: {search:?}",
                    case[0]
                );
            } else {
                assert!(
                    search
                        .paths
                        .iter()
                        .any(|p| p.report.state() == &PathState::Valid
                            && p.report.revocation()
                                == [RevocationOutcome::Good, RevocationOutcome::Good]),
                    "{}: {search:?}",
                    case[0]
                );
            }
        }
    }
    assert_eq!(count, expected);
}

#[test]
fn deferred_crl_semantics_cannot_mask_an_unverified_target() {
    check_security_cases(|case| case[2] == "Signature", 1);
}
#[test]
fn deferred_crl_semantics_cannot_authorize_an_unverified_crl_signer() {
    check_security_cases(|case| case[0].ends_with("critical-ancestor"), 2);
}
#[test]
fn unauthenticated_names_cannot_block_other_candidates() {
    check_security_cases(|case| case[0] == "clean" || case[0].starts_with("junk-"), 7);
}

#[test]
fn unsupported_name_edges_preserve_exact_paths_and_indeterminate_dead_ends() {
    check_security_cases(
        |case| case[0] == "opaque-anchor" || case[2] == "NameForm",
        2,
    );
}

#[test]
fn junk_relative_distribution_points_do_not_block_other_paths() {
    let data = security_data();
    let limits = Limits::default();
    let document = Document::parse(&data.certificates["junk-relative"], &limits).unwrap();
    let certificate = Certificate::parse(document.root()).unwrap();
    let engine = engine::Engine {
        certificates: &[],
        crls: &[],
        cache: Vec::new(),
        time: time(),
    };
    let outcome = engine.status(&certificate, &mut Budget::new(&limits));
    assert_eq!(
        outcome.unwrap(),
        RevocationOutcome::Unknown(UnknownReason::NameForm)
    );
    check_security_cases(|case| case[0].starts_with("relative-point-"), 2);
}

#[test]
fn crl_issuer_filtering_avoids_cross_product_signature_checks() {
    let data = security_data();
    let case = data
        .cases
        .iter()
        .find(|case| case[0] == "signer-filtering")
        .unwrap();
    let limits = Limits::default();
    let documents = case[4]
        .split(',')
        .map(|name| Document::parse(&data.certificates[name], &limits).unwrap())
        .collect::<Vec<_>>();
    let certificates = documents
        .iter()
        .map(|document| Certificate::parse(document.root()).unwrap())
        .collect::<Vec<_>>();
    let documents = case[5]
        .split(',')
        .map(|name| Document::parse(&data.crls[name], &limits).unwrap())
        .collect::<Vec<_>>();
    let crls = documents
        .iter()
        .map(|document| CertificateList::parse(document.root()).unwrap())
        .collect::<Vec<_>>();
    // The host anchor is separate from the ten supplied CAs and target.
    let anchor = TrustAnchor::from_certificate(&certificates[0]);
    let pool = certificates[1..].iter().collect::<Vec<_>>();
    let crls = crls.iter().collect::<Vec<_>>();
    assert_eq!((pool.len(), crls.len()), (11, 11));
    for mode in [
        crate::validation::SearchMode::FirstAcceptable,
        crate::validation::SearchMode::All,
    ] {
        let mut options = ValidationOptions::new(Time::from_utc(2025, 1, 1, 0, 0, 0).unwrap());
        options.search = mode;
        let mut calls = 0;
        let mut verifier = |input: SignatureInput<'_>, _: &Limits| {
            calls += 1;
            data.records
                .get(&key(input))
                .copied()
                .unwrap_or(SignatureResult::Unavailable)
        };
        let search = build_paths_with_crls(
            pool.len() - 1,
            &pool,
            &[anchor],
            &crls,
            &options,
            &mut verifier,
            &limits,
        );
        eprintln!("CRL issuer filtering {mode:?}: {calls} host verifications");
        assert!(search.complete, "{search:?}");
        assert!(
            search
                .paths
                .iter()
                .any(|p| p.report.state() == &PathState::Valid),
            "{search:?}"
        );
        // Eleven certificate signatures and eleven CRLs; memo hits add no calls.
        assert_eq!(calls, 22, "{mode:?}");
    }
}
