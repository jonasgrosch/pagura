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
        if path.extension().is_none_or(|extension| extension != "params") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let params: std::collections::BTreeMap<_, _> = raw.lines()
            .map(|line| line.split_once('=').unwrap()).collect();
        let data = fixture(area, &format!("{name}.jb2"));
        let globals = fixture(area, &format!("{name}.globals"));
        let result = decode(&data, Some(&globals), &Limits::default());
        if params["outcome"] == "valid" {
            assert_eq!(result.unwrap_or_else(|error| panic!("{name}: {error:?}")), Bitmap {
                width: params["width"].parse().unwrap(),
                height: params["height"].parse().unwrap(),
                data: fixture(area, &format!("{name}.out")),
            }, "{name}");
            valid += 1;
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Syntax, "{name}");
            rejected += 1;
        }
    }
    assert_eq!((valid, rejected), (5, 12));
}
