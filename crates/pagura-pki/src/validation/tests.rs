use super::*;
use crate::der::Document;
use std::collections::BTreeMap;
use std::process::Command;

fn unhex(text: &str) -> Vec<u8> {
    if text == "-" {
        return Vec::new();
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn time() -> Time<'static> {
    Time {
        year: 2020,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        fraction: "",
    }
}
fn record_key(input: SignatureInput<'_>) -> Vec<Vec<u8>> {
    let bits = |bits: BitString<'_>| {
        let mut encoded = vec![bits.unused_bits()];
        encoded.extend_from_slice(bits.as_bytes());
        encoded
    };
    vec![
        input.algorithm.element.encoded().to_vec(),
        input.message.to_vec(),
        bits(input.signature),
        input.issuer_key.algorithm.as_bytes().to_vec(),
        input
            .issuer_key
            .parameters
            .map_or_else(Vec::new, |p| p.encoded().to_vec()),
        bits(input.issuer_key.key),
    ]
}

struct PkitsData<'a> {
    encodings: BTreeMap<&'a str, Vec<u8>>,
    records: BTreeMap<Vec<Vec<u8>>, SignatureResult>,
    cases: Vec<Vec<&'a str>>,
}
impl<'a> PkitsData<'a> {
    fn read(text: &'a str) -> Self {
        let mut encodings = BTreeMap::new();
        let mut records = BTreeMap::new();
        let mut cases = Vec::new();
        for line in text.lines() {
            let fields = line.split('\t').collect::<Vec<_>>();
            match fields[0] {
                "CERT" => {
                    encodings.insert(fields[1], unhex(fields[2]));
                }
                "SIGNATURE" => {
                    records.insert(
                        fields[1..7].iter().map(|v| unhex(v)).collect::<Vec<_>>(),
                        match fields[7] {
                            "1" => SignatureResult::Verified,
                            "0" => SignatureResult::Invalid,
                            _ => SignatureResult::Unavailable,
                        },
                    );
                }
                "CASE" => cases.push(fields),
                _ => panic!("unexpected PKITS record"),
            }
        }
        Self {
            encodings,
            records,
            cases,
        }
    }
}
fn pkits_options(case: &[&str]) -> ValidationOptions<'static> {
    let mut options = ValidationOptions::new(time());
    options.explicit_policy = case[5] == "1";
    options.inhibit_mapping = case[6] == "1";
    options.inhibit_any_policy = case[7] == "1";
    options.policies = case[8]
        .split(',')
        .map(|n| {
            if n == "any" {
                Oid::any()
            } else {
                Oid(vec![96, 134, 72, 1, 101, 3, 2, 1, 48, n.parse().unwrap()])
            }
        })
        .collect();
    options
}

fn assert_record_binding(records: &BTreeMap<Vec<Vec<u8>>, SignatureResult>) {
    let sample = records.keys().next().unwrap();
    for coordinate in 0..6 {
        let mut altered = sample.clone();
        altered[coordinate].push(0);
        assert!(!records.contains_key(&altered));
    }
}

fn assert_pkits_path(
    pool: &[&Certificate<'_>],
    anchor: TrustAnchor<'_>,
    options: &ValidationOptions<'_>,
    verifier: &mut impl SignatureVerifier,
    case: &[&str],
) {
    let id = case[1];
    let expected = case[3] == "1";
    let direct = validate_path(pool, anchor, options, verifier, &Limits::default());
    assert_eq!(
        direct.basic == PathState::Valid,
        expected,
        "{id}: {direct:?}"
    );
    assert!(
        !matches!(direct.basic, PathState::Indeterminate { .. }),
        "{id}: {direct:?}"
    );
    if expected && case[9] != "?" {
        let policies = if case[9] == "-" {
            Vec::new()
        } else {
            case[9]
                .split(',')
                .map(|n| {
                    if n == "any" {
                        Oid::any()
                    } else {
                        Oid(vec![96, 134, 72, 1, 101, 3, 2, 1, 48, n.parse().unwrap()])
                    }
                })
                .collect()
        };
        assert_eq!(direct.policies, policies, "{id}: policy output");
    }
}

fn pkits_text() -> Option<String> {
    let output = Command::new("python3")
        .arg(crate::tests::generator("pkits.py"))
        .arg(crate::tests::references())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    if text.starts_with("ABSENT\t") {
        eprintln!("External PKITS suite absent; generated path validation tests still run");
        return None;
    }
    Some(text)
}

#[test]
fn cached_pkits_certification_paths() {
    let Some(text) = pkits_text() else {
        return;
    };
    let PkitsData {
        encodings,
        records,
        cases,
    } = PkitsData::read(&text);
    let limits = Limits::default();
    let documents = encodings
        .iter()
        .map(|(&name, data)| (name, Document::parse(data, &limits).unwrap()))
        .collect::<BTreeMap<_, _>>();
    let certificates = documents
        .iter()
        .map(|(&name, document)| (name, Certificate::parse(document.root()).unwrap()))
        .collect::<BTreeMap<_, _>>();
    assert_record_binding(&records);
    let mut verifier = |input: SignatureInput<'_>, _: &Limits| {
        records
            .get(&record_key(input))
            .copied()
            .unwrap_or(SignatureResult::Unavailable)
    };
    let mut counts = BTreeMap::<u8, [usize; 4]>::new();
    let mut failures = Vec::new();
    for case in cases {
        let id = case[1];
        let section = case[2];
        let expected = case[3] == "1";
        let revocation = case[4] == "1";
        let mut options = pkits_options(&case);
        options.search = SearchMode::All;
        let anchor = TrustAnchor::from_certificate(&certificates[case[10]]);
        let pool = case[11..]
            .iter()
            .map(|name| &certificates[name])
            .collect::<Vec<_>>();
        let search = build_paths(
            pool.len() - 1,
            &pool,
            &[anchor],
            &options,
            &mut verifier,
            &limits,
        );
        assert!(
            search.complete,
            "{id}: incomplete search {:?}",
            search.stopped
        );
        let basic_valid = search
            .paths
            .iter()
            .any(|p| p.report.basic == PathState::Valid);
        let unknown = search
            .paths
            .iter()
            .any(|p| matches!(p.report.basic, PathState::Indeterminate { .. }));
        if !revocation {
            assert_pkits_path(&pool, anchor, &options, &mut verifier, &case);
        }
        let count = counts.entry(section.parse().unwrap()).or_default();
        count[0] += 1;
        if revocation {
            assert!(
                search.paths.iter().all(|p| !matches!(
                    p.report.basic,
                    PathState::Indeterminate {
                        reason: Uncertainty::Signature,
                        ..
                    }
                )),
                "{id}: missing exact signature record"
            );
            count[2] += 1;
            if !basic_valid && !unknown {
                failures.push(format!(
                    "{id}: revocation case has no basically acceptable path: {search:?}"
                ));
            }
            assert!(
                !search
                    .paths
                    .iter()
                    .any(|p| p.report.state == PathState::Valid),
                "{id}: fabricated revocation success"
            );
        } else if basic_valid == expected && !unknown {
            count[1] += 1;
        } else {
            count[3] += 1;
            failures.push(format!("{id}: expected basic={expected}, {search:?}"));
        }
    }
    eprintln!("PKITS section: variants, basic matches, revocation pending, failures");
    for (section, count) in &counts {
        eprintln!("4.{section}: {count:?}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn synthetic() -> String {
    let output = Command::new("python3")
        .arg(crate::tests::generator("paths.py"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn structural_verifier(_: SignatureInput<'_>, _: &Limits) -> SignatureResult {
    SignatureResult::Verified
}
fn synthetic_options() -> ValidationOptions<'static> {
    let mut options = ValidationOptions::new(Time {
        year: 2025,
        ..time()
    });
    options.key_usage = 1; // digitalSignature when the target asserts keyUsage.
    options.extended_key_usage = Some(Oid(vec![43, 6, 1, 5, 5, 7, 3, 8]));
    options
}

#[test]
fn independent_path_constraints_and_updated_names() {
    for line in synthetic().lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        let bytes = fields[2..].iter().map(|s| unhex(s)).collect::<Vec<_>>();
        let limits = Limits::default();
        let documents = bytes
            .iter()
            .map(|b| Document::parse(b, &limits).unwrap())
            .collect::<Vec<_>>();
        let certificates = documents
            .iter()
            .map(|d| Certificate::parse(d.root()).unwrap())
            .collect::<Vec<_>>();
        let anchor = TrustAnchor::from_certificate(&certificates[0]);
        let result = validate_path(
            &[&certificates[1], &certificates[2]],
            anchor,
            &synthetic_options(),
            &mut structural_verifier,
            &limits,
        );
        match (&result.basic, fields[1]) {
            (PathState::Valid, "valid") => assert!(matches!(
                result.state,
                PathState::Indeterminate {
                    reason: Uncertainty::Revocation,
                    ..
                }
            )),
            (
                PathState::Indeterminate {
                    reason: Uncertainty::NameForm,
                    ..
                },
                "NameForm",
            ) => (),
            (PathState::Invalid { reason, .. }, expected) => assert!(
                format!("{reason:?}").starts_with(expected),
                "{}: {result:?}",
                fields[0]
            ),
            _ => panic!("{}: expected {}, {result:?}", fields[0], fields[1]),
        }
    }
}

#[test]
fn host_trust_revocation_and_search_bounds() {
    let data = synthetic();
    let fields = data.lines().next().unwrap().split('\t').collect::<Vec<_>>();
    let bytes = fields[2..].iter().map(|s| unhex(s)).collect::<Vec<_>>();
    let limits = Limits::default();
    let documents = bytes
        .iter()
        .map(|b| Document::parse(b, &limits).unwrap())
        .collect::<Vec<_>>();
    let certs = documents
        .iter()
        .map(|d| Certificate::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let anchor = TrustAnchor::from_certificate(&certs[0]);
    let mut options = synthetic_options();
    options.search = SearchMode::All;
    let pool = [&certs[0], &certs[1], &certs[2]];
    let search = build_paths(2, &pool, &[], &options, &mut structural_verifier, &limits);
    assert!(search.complete);
    assert!(search.paths.iter().all(|p| matches!(
        p.report.state,
        PathState::Invalid {
            reason: InvalidReason::Untrusted,
            ..
        }
    )));
    let search = build_paths(
        2,
        &pool,
        &[anchor],
        &options,
        &mut structural_verifier,
        &limits,
    );
    assert!(search.complete);
    assert_eq!(search.paths.len(), 1);
    assert_eq!(search.paths[0].certificates, [1, 2]);
    assert!(matches!(
        search.paths[0].report.state(),
        PathState::Indeterminate {
            reason: Uncertainty::Revocation,
            ..
        }
    ));
    for bounded in [
        Limits { depth: 1, ..limits },
        Limits {
            value_nodes: 1,
            ..limits
        },
        Limits {
            operations: 1,
            ..limits
        },
        Limits {
            document_bytes: 1,
            ..limits
        },
        Limits {
            string_bytes: 1,
            ..limits
        },
    ] {
        let search = build_paths(
            2,
            &pool,
            &[anchor],
            &options,
            &mut structural_verifier,
            &bounded,
        );
        assert!(!search.complete, "{bounded:?}");
        assert!(matches!(search.stopped, Some(Uncertainty::Limit(_))));
    }
    let mut unknown = |_: SignatureInput<'_>, _: &Limits| SignatureResult::Unavailable;
    let result = validate_path(&pool[1..], anchor, &options, &mut unknown, &limits);
    assert!(matches!(
        result.state,
        PathState::Indeterminate {
            reason: Uncertainty::Signature,
            ..
        }
    ));
    let expired = ValidationOptions::new(Time {
        year: 2031,
        ..time()
    });
    assert!(matches!(
        validate_path(&pool[1..], anchor, &expired, &mut unknown, &limits).state,
        PathState::Invalid {
            reason: InvalidReason::Expired,
            ..
        }
    ));
}

#[test]
fn policy_graph_shares_parents_and_bounds_edges() {
    let limits = Limits {
        value_nodes: 5000,
        ..Limits::default()
    };
    let mut budget = budget::Budget::new(&limits);
    let options = ValidationOptions {
        explicit_policy: true,
        ..ValidationOptions::new(time())
    };
    let ids = (1..=3).map(|n| Oid(vec![42, 3, n])).collect::<Vec<_>>();
    let mappings = ids
        .iter()
        .flat_map(|a| ids.iter().map(move |b| (a.clone(), b.clone())))
        .collect::<Vec<_>>();
    let mut graph = policy::Graph::new(40, &options, &mut budget).unwrap();
    for depth in 1..=40 {
        graph
            .process(Some(&ids), depth, false, depth == 40, &mut budget)
            .unwrap();
        if depth != 40 {
            graph.mappings(&mappings, depth, &mut budget).unwrap();
        }
    }
    assert_eq!(graph.finish(&ids, 40, &mut budget).unwrap(), ids);
    let limits = Limits {
        value_nodes: 5,
        ..limits
    };
    let mut budget = budget::Budget::new(&limits);
    let mut graph = policy::Graph::new(40, &options, &mut budget).unwrap();
    assert!(matches!(
        graph.process(Some(&ids), 1, false, false, &mut budget),
        Err(Issue::Unknown(Uncertainty::Limit(_)))
    ));
}

#[test]
fn issuer_graphs_keep_each_path_state_and_bound_branching() {
    let output = Command::new("python3")
        .arg(crate::tests::generator("graphs.py"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let bytes = text
        .lines()
        .map(|line| unhex(line.split('\t').nth(1).unwrap()))
        .collect::<Vec<_>>();
    let limits = Limits::default();
    let documents = bytes
        .iter()
        .map(|b| Document::parse(b, &limits).unwrap())
        .collect::<Vec<_>>();
    let certificates = documents
        .iter()
        .map(|d| Certificate::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let anchor = TrustAnchor::from_certificate(&certificates[0]);
    let pool = certificates.iter().collect::<Vec<_>>();
    let mut options = synthetic_options();
    options.search = SearchMode::All;
    let result = build_paths(
        3,
        &pool[..7],
        &[anchor],
        &options,
        &mut structural_verifier,
        &limits,
    );
    assert!(result.complete);
    assert_eq!(result.paths.len(), 2);
    assert_eq!(
        result
            .paths
            .iter()
            .filter(|p| p.report.basic == PathState::Valid)
            .count(),
        1
    );
    assert!(result.paths.iter().any(|p| matches!(
        p.report.basic,
        PathState::Invalid {
            reason: InvalidReason::CriticalExtension(_),
            ..
        }
    )));
    let cycle = build_paths(
        6,
        &pool[..7],
        &[anchor],
        &options,
        &mut structural_verifier,
        &limits,
    );
    assert!(cycle.complete);
    assert_eq!(cycle.paths.len(), 1);
    assert!(matches!(
        cycle.paths[0].report.state,
        PathState::Invalid {
            reason: InvalidReason::Untrusted,
            ..
        }
    ));
    let unknown_name = build_paths(
        3,
        &pool[..8],
        &[anchor],
        &options,
        &mut structural_verifier,
        &limits,
    );
    assert!(unknown_name.complete);
    assert_eq!(unknown_name.stopped, None);
    assert!(
        unknown_name
            .paths
            .iter()
            .any(|p| p.report.basic == PathState::Valid)
    );
    let bounded = Limits {
        value_nodes: 300,
        ..limits
    };
    let mut branches = pool[..7].to_vec();
    branches.extend_from_slice(&pool[8..]);
    let result = build_paths(
        3,
        &branches,
        &[anchor],
        &options,
        &mut structural_verifier,
        &bounded,
    );
    assert!(!result.complete);
    assert!(matches!(result.stopped, Some(Uncertainty::Limit(_))));
    assert!(result.paths.len() <= bounded.value_nodes);
}

#[test]
fn caller_time_and_purpose_inputs_are_checked() {
    let data = synthetic();
    let line = data
        .lines()
        .find(|line| line.starts_with("target-key-usage\t"))
        .unwrap();
    let bytes = line.split('\t').skip(2).map(unhex).collect::<Vec<_>>();
    let limits = Limits::default();
    let documents = bytes
        .iter()
        .map(|b| Document::parse(b, &limits).unwrap())
        .collect::<Vec<_>>();
    let certs = documents
        .iter()
        .map(|d| Certificate::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let anchor = TrustAnchor::from_certificate(&certs[0]);
    let pool = [&certs[1], &certs[2]];
    let mut options = synthetic_options();
    options.key_usage = 0;
    assert!(matches!(
        validate_path(&pool, anchor, &options, &mut structural_verifier, &limits).basic,
        PathState::Invalid {
            reason: InvalidReason::KeyUsage,
            ..
        }
    ));
    options.extended_key_usage = Some(Oid(vec![42, 3, 4]));
    assert!(matches!(
        validate_path(&pool, anchor, &options, &mut structural_verifier, &limits).basic,
        PathState::Indeterminate {
            reason: Uncertainty::KeyUsagePurpose,
            ..
        }
    ));
    options.key_usage = 1 << 2; // Private-purpose application explicitly requires keyEncipherment.
    assert_eq!(
        validate_path(&pool, anchor, &options, &mut structural_verifier, &limits).basic,
        PathState::Valid
    );
    options.time.month = 13;
    assert!(matches!(
        validate_path(&pool, anchor, &options, &mut structural_verifier, &limits).basic,
        PathState::Invalid {
            reason: InvalidReason::Input,
            ..
        }
    ));
    assert!(matches!(
        validate_path(&[], anchor, &options, &mut structural_verifier, &limits).basic,
        PathState::Invalid {
            reason: InvalidReason::Input,
            ..
        }
    ));
}

#[test]
fn validity_endpoints_are_inclusive_and_fractional_time_is_exact() {
    let data = synthetic();
    let fields = data.lines().next().unwrap().split('\t').collect::<Vec<_>>();
    let bytes = fields[2..].iter().map(|s| unhex(s)).collect::<Vec<_>>();
    let limits = Limits::default();
    let documents = bytes
        .iter()
        .map(|b| Document::parse(b, &limits).unwrap())
        .collect::<Vec<_>>();
    let certs = documents
        .iter()
        .map(|d| Certificate::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let anchor = TrustAnchor::from_certificate(&certs[0]);
    let path = [&certs[1], &certs[2]];
    for evaluation in [certs[2].not_before, certs[2].not_after] {
        let options = ValidationOptions::new(evaluation);
        assert_eq!(
            validate_path(&path, anchor, &options, &mut structural_verifier, &limits).basic,
            PathState::Valid
        );
    }
    let after = ValidationOptions::new(Time {
        fraction: "0001",
        ..certs[2].not_after
    });
    assert!(matches!(
        validate_path(&path, anchor, &after, &mut structural_verifier, &limits).basic,
        PathState::Invalid {
            reason: InvalidReason::Expired,
            ..
        }
    ));
    let before = ValidationOptions::new(Time {
        year: 2023,
        ..certs[2].not_before
    });
    assert!(matches!(
        validate_path(&path, anchor, &before, &mut structural_verifier, &limits).basic,
        PathState::Invalid {
            reason: InvalidReason::NotYetValid,
            ..
        }
    ));
}

#[test]
fn initial_empty_permitted_sets_deny_only_the_selected_forms() {
    let data = synthetic();
    for id in ["host-anchor", "dns-descendant", "smtp-utf8"] {
        let line = data
            .lines()
            .find(|line| line.starts_with(&format!("{id}\t")))
            .unwrap();
        let bytes = line.split('\t').skip(2).map(unhex).collect::<Vec<_>>();
        let limits = Limits::default();
        let documents = bytes
            .iter()
            .map(|b| Document::parse(b, &limits).unwrap())
            .collect::<Vec<_>>();
        let certs = documents
            .iter()
            .map(|d| Certificate::parse(d.root()).unwrap())
            .collect::<Vec<_>>();
        let anchor = TrustAnchor::from_certificate(&certs[0]);
        let path = [&certs[1], &certs[2]];
        for form in [NameForm::Dns, NameForm::Mailbox] {
            let mut options = synthetic_options();
            options.denied_name_forms = vec![form];
            let actual = validate_path(&path, anchor, &options, &mut structural_verifier, &limits);
            let denied = (id == "dns-descendant" && form == NameForm::Dns)
                || (id == "smtp-utf8" && form == NameForm::Mailbox);
            assert_eq!(
                actual.basic == PathState::Valid,
                !denied,
                "{id}/{form:?}: {actual:?}"
            );
            if denied {
                assert!(matches!(
                    actual.basic,
                    PathState::Invalid {
                        reason: InvalidReason::NameConstraint,
                        ..
                    }
                ));
            }
        }
    }
}

#[test]
fn signature_input_work_is_admitted_before_calling_the_host() {
    let data = synthetic();
    let bytes = data
        .lines()
        .next()
        .unwrap()
        .split('\t')
        .skip(2)
        .map(unhex)
        .collect::<Vec<_>>();
    let limits = Limits::default();
    let documents = bytes
        .iter()
        .map(|b| Document::parse(b, &limits).unwrap())
        .collect::<Vec<_>>();
    let certs = documents
        .iter()
        .map(|d| Certificate::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let anchor = TrustAnchor::from_certificate(&certs[0]);
    let path = [&certs[1], &certs[2]];
    let mut calls = 0;
    let mut verifier = |_: SignatureInput<'_>, _: &Limits| {
        calls += 1;
        SignatureResult::Verified
    };
    let bounded = Limits {
        operations: 128,
        ..limits
    };
    let result = validate_path(&path, anchor, &synthetic_options(), &mut verifier, &bounded);
    assert!(matches!(
        result.basic,
        PathState::Indeterminate {
            reason: Uncertainty::Limit("pki.operations"),
            ..
        }
    ));
    assert_eq!(calls, 0);
}

fn input_fixtures() -> String {
    let output = Command::new("python3")
        .arg(crate::tests::generator("path_inputs.py"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn conservative_names_and_unconstrained_sans() {
    let limits = Limits::default();
    for line in input_fixtures()
        .lines()
        .filter(|line| line.starts_with("CASE"))
    {
        let fields = line.split('\t').collect::<Vec<_>>();
        let bytes = fields[3..].iter().map(|s| unhex(s)).collect::<Vec<_>>();
        let documents = bytes
            .iter()
            .map(|b| Document::parse(b, &limits).unwrap())
            .collect::<Vec<_>>();
        let certificates = documents
            .iter()
            .map(|d| Certificate::parse(d.root()).unwrap())
            .collect::<Vec<_>>();
        let result = validate_path(
            &[&certificates[1], &certificates[2]],
            TrustAnchor::from_certificate(&certificates[0]),
            &ValidationOptions::new(Time {
                year: 2025,
                ..time()
            }),
            &mut structural_verifier,
            &limits,
        );
        let matches = match (&result.basic, fields[2]) {
            (PathState::Valid, "valid")
            | (
                PathState::Indeterminate {
                    reason: Uncertainty::NameForm,
                    ..
                },
                "NameForm",
            ) => true,
            (PathState::Invalid { reason, .. }, expected) => format!("{reason:?}") == expected,
            _ => false,
        };
        assert!(matches, "{}: {result:?}", fields[1]);
    }
}
#[test]
fn padded_issuer_pool_memoizes_and_caps_host_verification() {
    let limits = Limits::default();
    let text = input_fixtures();
    let line = text.lines().find(|line| line.starts_with("GRAPH")).unwrap();
    let bytes = line.split('\t').skip(1).map(unhex).collect::<Vec<_>>();
    let documents = bytes
        .iter()
        .map(|b| Document::parse(b, &limits).unwrap())
        .collect::<Vec<_>>();
    let certs = documents
        .iter()
        .map(|d| Certificate::parse(d.root()).unwrap())
        .collect::<Vec<_>>();
    let pool = certs.iter().collect::<Vec<_>>();
    let anchor = TrustAnchor::from_certificate(&certs[0]);
    let mut options = ValidationOptions::new(Time {
        year: 2025,
        ..time()
    });
    let mut calls = 0;
    let mut verifier = |_: SignatureInput<'_>, _: &Limits| {
        calls += 1;
        SignatureResult::Verified
    };
    let first = build_paths(
        pool.len() - 1,
        &pool,
        &[anchor],
        &options,
        &mut verifier,
        &limits,
    );
    assert!(first.complete && !first.exhaustive);
    assert_eq!(first.paths.len(), 1);
    assert_eq!(calls, 2);
    options.search = SearchMode::All;
    calls = 0;
    let mut verifier = |_: SignatureInput<'_>, _: &Limits| {
        calls += 1;
        SignatureResult::Verified
    };
    let full = build_paths(
        pool.len() - 1,
        &pool,
        &[anchor],
        &options,
        &mut verifier,
        &limits,
    );
    assert_eq!(calls, 100);
    assert!(!full.complete && !full.exhaustive);
    assert_eq!(full.stopped, Some(Uncertainty::Limit("pki.signatures")));
    assert!(matches!(
        full.paths.last().unwrap().report.basic,
        PathState::Indeterminate {
            reason: Uncertainty::Limit("pki.signatures"),
            ..
        }
    ));
    let bounded = Limits {
        operations: 500_000,
        ..limits
    };
    calls = 0;
    let mut verifier = |_: SignatureInput<'_>, _: &Limits| {
        calls += 1;
        SignatureResult::Verified
    };
    let small = build_paths(
        pool.len() - 1,
        &pool,
        &[anchor],
        &options,
        &mut verifier,
        &bounded,
    );
    assert_eq!(calls, 1);
    assert!(!small.complete);
}

#[test]
fn signature_memo_binds_all_inputs_and_preserves_unavailable_results() {
    let fixture = synthetic();
    let bytes = unhex(fixture.lines().next().unwrap().split('\t').nth(3).unwrap());
    let limits = Limits::default();
    let document = Document::parse(&bytes, &limits).unwrap();
    let certificate = Certificate::parse(document.root()).unwrap();
    let zero_bytes = [3, 2, 0, 0];
    let one_bytes = [3, 2, 0, 1];
    let padding_bytes = [3, 2, 1, 0];
    let parameter_bytes = [2, 1, 0];
    let zero = Document::parse(&zero_bytes, &limits).unwrap();
    let one = Document::parse(&one_bytes, &limits).unwrap();
    let padding = Document::parse(&padding_bytes, &limits).unwrap();
    let parameters = Document::parse(&parameter_bytes, &limits).unwrap();
    let zero = BitString::parse(zero.root()).unwrap();
    let one = BitString::parse(one.root()).unwrap();
    let padding = BitString::parse(padding.root()).unwrap();
    let input = SignatureInput {
        algorithm: certificate.signature_algorithm,
        message: certificate.tbs.encoded(),
        signature: zero,
        issuer_key: WorkingPublicKey {
            key: zero,
            ..path::key(certificate.public_key)
        },
    };
    let variants = [
        input,
        SignatureInput {
            algorithm: certificate.public_key.algorithm,
            ..input
        },
        SignatureInput {
            message: b"different signed object",
            ..input
        },
        SignatureInput {
            signature: one,
            ..input
        },
        SignatureInput {
            signature: padding,
            ..input
        },
        SignatureInput {
            issuer_key: WorkingPublicKey {
                algorithm: certificate.signature_algorithm.algorithm,
                ..input.issuer_key
            },
            ..input
        },
        SignatureInput {
            issuer_key: WorkingPublicKey {
                parameters: Some(parameters.root()),
                ..input.issuer_key
            },
            ..input
        },
        SignatureInput {
            issuer_key: WorkingPublicKey {
                key: one,
                ..input.issuer_key
            },
            ..input
        },
        SignatureInput {
            issuer_key: WorkingPublicKey {
                key: padding,
                ..input.issuer_key
            },
            ..input
        },
    ];
    let mut budget = budget::Budget::new(&limits);
    let mut calls = 0;
    let mut verifier = |_: SignatureInput<'_>, _: &Limits| {
        calls += 1;
        [
            SignatureResult::Verified,
            SignatureResult::Invalid,
            SignatureResult::Unavailable,
        ][calls % 3]
    };
    let results = variants
        .iter()
        .map(|&input| budget.verify(input, &mut verifier).unwrap())
        .collect::<Vec<_>>();
    for (&input, expected) in variants.iter().zip(results) {
        assert_eq!(budget.verify(input, &mut verifier).unwrap(), expected);
    }
    assert_eq!(calls, variants.len());
}
