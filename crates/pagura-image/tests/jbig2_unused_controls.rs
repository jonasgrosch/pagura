#![forbid(unsafe_code)]
//! Independent matrices and hostile vectors precede any recovery policy.
use pagura_core::{ErrorKind, Limits};
use pagura_image::jbig2::{Bitmap, decode};
use pagura_testkit::{fixture, fixture_path};

#[test]
fn independent_strict_twins_and_hostile_controls() {
    let area = "recovery_jbig2_tables";
    let mut valid = 0;
    let mut rejected = 0;
    for entry in std::fs::read_dir(fixture_path(area, "")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_none_or(|extension| extension != "params")
        {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let params: std::collections::BTreeMap<_, _> = raw
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let data = fixture(area, &format!("{name}.jb2"));
        let globals = fixture(area, &format!("{name}.globals"));
        let result = decode(&data, Some(&globals), &Limits::default());
        if params["outcome"] == "valid" {
            assert_eq!(
                result.unwrap_or_else(|error| panic!("{name}: {error:?}")),
                Bitmap {
                    width: params["width"].parse().unwrap(),
                    height: params["height"].parse().unwrap(),
                    data: fixture(area, &format!("{name}.out")),
                },
                "{name}"
            );
            valid += 1;
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax, "{name}");
            rejected += 1;
        }
    }
    assert_eq!((valid, rejected), (7, 16));
}

#[test]
fn only_unused_selectors_are_reported_and_tables_stay_authoritative() {
    use pagura_core::{Diagnostics, Position, Recovery};
    use pagura_image::jbig2::{decode_with_diagnostics, repairs::UNUSED_AGGREGATION_TABLE};
    let area = "recovery_jbig2_tables";
    let mut rendered = 0;
    let mut refused = 0;
    for entry in std::fs::read_dir(fixture_path(area, "")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_none_or(|extension| extension != "params")
        {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let params: std::collections::BTreeMap<_, _> = raw
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let data = fixture(area, &format!("{name}.jb2"));
        let globals = fixture(area, &format!("{name}.globals"));
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let result =
            decode_with_diagnostics(&data, Some(&globals), &Limits::default(), &mut diagnostics);
        if params["outcome"] == "valid" || params.contains_key("repair") {
            assert_eq!(
                result.unwrap_or_else(|error| panic!("{name}: {error:?}")),
                Bitmap {
                    width: params["width"].parse().unwrap(),
                    height: params["height"].parse().unwrap(),
                    data: fixture(area, &format!("{name}.out")),
                },
                "{name}"
            );
            rendered += 1;
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax, "{name}");
            refused += 1;
        }
        if params.contains_key("repair") || params["repair_before_error"] == "1" {
            assert_eq!(diagnostics.as_slice().len(), 1, "{name}");
            assert_eq!(
                diagnostics.as_slice()[0].repair(),
                Some(UNUSED_AGGREGATION_TABLE),
                "{name}"
            );
        } else {
            assert!(diagnostics.is_empty(), "{name}");
        }
        if let Some(offset) = params.get("offset") {
            let position = Position::Buffer(offset.parse().unwrap());
            assert_eq!(diagnostics.as_slice()[0].position, position, "{name}");
            let mut strict = Diagnostics::new(Recovery::Strict);
            let error =
                decode_with_diagnostics(&data, Some(&globals), &Limits::default(), &mut strict)
                    .unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
            assert_eq!(error.repair, Some(UNUSED_AGGREGATION_TABLE), "{name}");
            assert_eq!(error.position, position, "{name}");
            assert!(strict.is_empty(), "{name}");
            for limits in [
                Limits {
                    decode_work: 0,
                    ..Limits::default()
                },
                Limits {
                    decoded_bytes: 0,
                    ..Limits::default()
                },
                Limits {
                    objects: 0,
                    ..Limits::default()
                },
            ] {
                let mut diagnostics = Diagnostics::new(Recovery::Repair);
                assert_eq!(
                    decode_with_diagnostics(&data, Some(&globals), &limits, &mut diagnostics)
                        .unwrap_err()
                        .kind,
                    ErrorKind::Limit,
                    "{name}"
                );
            }
        }
    }
    assert_eq!((rendered, refused), (13, 10));
}

#[test]
fn repeated_selectors_keep_the_first_location_when_a_sink_is_reused() {
    use pagura_core::{Diagnostics, Position, Recovery};
    use pagura_image::jbig2::{decode_with_diagnostics, repairs::UNUSED_AGGREGATION_TABLE};
    let area = "recovery_jbig2_tables";
    for name in [
        "native-repeated-main-unused",
        "native-repeated-both-unused",
        "native-hostile-repeated-missing-table",
    ] {
        let raw = std::fs::read_to_string(fixture_path(area, &format!("{name}.params"))).unwrap();
        let params: std::collections::BTreeMap<_, _> = raw
            .lines()
            .map(|line| line.split_once('=').unwrap())
            .collect();
        let position = Position::Buffer(
            params
                .get("offset")
                .or_else(|| params.get("first_repair_offset"))
                .unwrap()
                .parse()
                .unwrap(),
        );
        let data = fixture(area, &format!("{name}.jb2"));
        let globals = fixture(area, &format!("{name}.globals"));
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        for _ in 0..4 {
            let result = decode_with_diagnostics(
                &data,
                Some(&globals),
                &Limits::default(),
                &mut diagnostics,
            );
            if params["repair_before_error"] == "1" {
                assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax, "{name}");
            } else {
                assert_eq!(
                    result.unwrap().data,
                    fixture(area, &format!("{name}.out")),
                    "{name}"
                );
            }
            assert_eq!(diagnostics.as_slice().len(), 1, "{name}");
            assert_eq!(
                diagnostics.as_slice()[0].repair(),
                Some(UNUSED_AGGREGATION_TABLE),
                "{name}"
            );
            assert_eq!(diagnostics.as_slice()[0].position, position, "{name}");
        }
        let mut strict = Diagnostics::new(Recovery::Strict);
        let error = decode_with_diagnostics(&data, Some(&globals), &Limits::default(), &mut strict)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{name}");
        assert_eq!(error.repair, Some(UNUSED_AGGREGATION_TABLE), "{name}");
        assert_eq!(error.position, position, "{name}");
        assert!(strict.is_empty(), "{name}");
        assert_eq!(diagnostics.take().len(), 1);
        let result =
            decode_with_diagnostics(&data, Some(&globals), &Limits::default(), &mut diagnostics);
        if params["repair_before_error"] == "1" {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax, "{name}");
        } else {
            assert_eq!(
                result.unwrap().data,
                fixture(area, &format!("{name}.out")),
                "{name}"
            );
        }
        assert_eq!(diagnostics.as_slice().len(), 1, "{name}");
        assert_eq!(diagnostics.as_slice()[0].position, position, "{name}");
    }
}
