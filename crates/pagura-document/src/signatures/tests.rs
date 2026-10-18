use super::*;
use crate::{Document, OpenOptions};
use pagura_core::{ErrorKind, Limits, Recovery};
use pagura_testkit::{fixture, fixture_path};
use std::fmt::Write;

fn open(name: &str, limits: Limits) -> Document {
    Document::open(
        fixture("signatures", &format!("{name}.pdf")),
        &OpenOptions {
            limits,
            ..OpenOptions::default()
        },
    )
    .unwrap()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}
fn permission(p: MdpPermission) -> u8 {
    match p {
        MdpPermission::NoChanges => 1,
        MdpPermission::FormsAndSignatures => 2,
        MdpPermission::FormsSignaturesAndAnnotations => 3,
    }
}
fn selection(s: &FieldSelection<'_>) -> String {
    match s {
        FieldSelection::All => "All".into(),
        FieldSelection::Include(v) => format!(
            "Include:{}",
            v.iter()
                .map(|s| String::from_utf8_lossy(s))
                .collect::<Vec<_>>()
                .join(",")
        ),
        FieldSelection::Exclude(v) => format!(
            "Exclude:{}",
            v.iter()
                .map(|s| String::from_utf8_lossy(s))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}
fn snapshot(name: &str, sig: &Signature<'_>, lock: Option<&FieldLock<'_>>) -> String {
    let (revision, later) = match &sig.results.revision_coverage {
        RevisionCoverage::Covered {
            index,
            later_revisions,
            ..
        } => (index.to_string(), later_revisions.len()),
        other => (format!("{other:?}"), 0),
    };
    let integrity = match sig.results.integrity {
        Integrity::InvalidByteRange(e) => format!("{e:?}"),
        other => format!("{other:?}"),
    };
    let ranges = sig.byte_range.as_ref().map_or_else(
        || "_".into(),
        |r| {
            let [a, b] = r.ranges();
            format!("{},{},{},{}", a.start, a.len(), b.start, b.len())
        },
    );
    let refs = sig
        .references
        .iter()
        .map(|r| match &r.transform {
            Transform::DocMdp(p) => format!("DocMDP:{}", permission(*p)),
            Transform::FieldMdp(s) => format!("FieldMDP:{}", selection(s)),
            Transform::UsageRights => "UR".into(),
            Transform::Other(n) => String::from_utf8_lossy(n.as_bytes()).into_owned(),
        })
        .collect::<Vec<_>>()
        .join(";");
    let lock = lock.map_or_else(
        || "_".into(),
        |l| {
            format!(
                "{}:{}",
                selection(&l.fields),
                l.permission
                    .map_or_else(|| "_".into(), |p| permission(p).to_string())
            )
        },
    );
    let metadata = if [sig.signing_time, sig.name, sig.reason, sig.location]
        .iter()
        .all(Option::is_none)
    {
        "_".into()
    } else {
        [sig.signing_time, sig.name, sig.reason, sig.location]
            .iter()
            .map(|s| String::from_utf8_lossy(s.unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        "{name}|{integrity}|{revision}|{later}|{:?}|{ranges}|{}|{}|{lock}|{metadata}",
        sig.results.algorithms,
        hex(sig.contents),
        if refs.is_empty() { "_" } else { &refs }
    )
}

#[test]
fn independent_structures() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_path("signatures", "")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|s| s != "expect") {
            continue;
        }
        count += 1;
        let name = path.file_stem().unwrap().to_str().unwrap();
        let expected = std::fs::read_to_string(&path).unwrap();
        let doc = open(name, Limits::default());
        let result = doc.signatures();
        if expected.starts_with("field-error|") {
            assert_eq!(
                format!(
                    "field-error|Approval|{:?}\n",
                    result.unwrap().fields[0].error.as_ref().unwrap().kind
                ),
                expected,
                "{name}"
            );
            continue;
        }
        let result = result.unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut lines = Vec::new();
        for f in &result.fields {
            lines.push(f.signature.as_ref().map_or_else(
                || format!("{}|unsigned", f.qualified_name),
                |s| snapshot(&f.qualified_name, s, f.lock.as_ref()),
            ));
        }
        if let Some(s) = &result.usage_rights {
            lines.push(snapshot("@UR3", s, None));
        }
        assert_eq!(lines.join("\n") + "\n", expected, "{name}");
        for signature in result
            .fields
            .iter()
            .filter_map(|f| f.signature.as_ref())
            .chain(result.usage_rights.iter())
        {
            match &signature.results.revision_coverage {
                RevisionCoverage::Covered { index: 0, .. } => assert!(matches!(
                    &signature.results.permitted_modifications,
                    PermittedModifications::Analysed(ModificationAnalysis {
                        outcome: ModificationOutcome::NoChanges,
                        ..
                    })
                )),
                RevisionCoverage::Covered { .. } => assert!(matches!(
                    &signature.results.permitted_modifications,
                    PermittedModifications::Analysed(_)
                )),
                RevisionCoverage::RepairedDocument | RevisionCoverage::Indeterminate => assert_eq!(
                    signature.results.permitted_modifications,
                    PermittedModifications::Inconclusive
                ),
                _ => assert_eq!(
                    signature.results.permitted_modifications,
                    PermittedModifications::NotAnalysedYet
                ),
            }
            assert_eq!(signature.results.signer_trust, SignerTrust::NotCheckedYet);
            assert_eq!(
                signature.results.validation_time,
                ValidationTime::NotCheckedYet
            );
            assert_eq!(
                signature.results.revocation,
                RevocationStatus::NotCheckedYet
            );
            if let Some(ranges) = &signature.byte_range {
                assert_eq!(ranges.signed_bytes().len(), 2);
                for (chunk, range) in ranges.signed_bytes().zip(ranges.ranges()) {
                    let original = &doc.original_bytes()[range.clone()];
                    assert!(std::ptr::eq(chunk, original));
                    assert_eq!(chunk, original);
                }
            }
            if matches!(
                signature.sub_filter,
                SubFilter::X509RsaSha1 | SubFilter::Other(_) | SubFilter::Unspecified
            ) {
                assert!(signature.cms_blob().is_none());
            } else {
                assert_eq!(signature.cms_blob(), Some(signature.contents));
            }
        }
    }
    assert!(count >= 40, "all independent fixtures must be exercised");
}

#[test]
fn budgets_fail_without_partial_results() {
    let doc = open("multiple-revisions", Limits::default());
    // Read source objects first so result limits, rather than lazy cache
    // admission, are exercised independently.
    doc.signatures().unwrap();
    let mut options = doc.options.clone();
    options.limits.value_nodes = 1;
    let mut limited = doc;
    limited.options = options;
    assert_eq!(limited.signatures().unwrap_err().kind, ErrorKind::Limit);
    let mut doc = open("multiple-revisions", Limits::default());
    doc.signatures().unwrap();
    doc.options.limits.document_bytes = 1;
    assert_eq!(doc.signatures().unwrap_err().kind, ErrorKind::Limit);
    let mut doc = open("multiple-revisions", Limits::default());
    doc.signatures().unwrap();
    doc.options.limits.file_bytes = 1;
    assert_eq!(doc.signatures().unwrap_err().kind, ErrorKind::Limit);
}

#[test]
fn checked_ranges_do_not_depend_on_recovery_mode() {
    let bytes = fixture("signatures", "structure.pdf");
    let doc = Document::open(
        bytes,
        &OpenOptions {
            recovery: Recovery::Strict,
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let result = doc.signatures().unwrap();
    assert_eq!(
        result.fields[0]
            .signature
            .as_ref()
            .unwrap()
            .results
            .integrity,
        Integrity::ByteRangeCheckedCryptographyNotChecked
    );
    assert_eq!(doc.diagnostics(), []);
}
