use super::{Bench, Command, Conversion, Output, Source, Timings, parse};
use pagura::{FastParams, OverprintPreview, Recovery, RenderOptions};
use std::ffi::OsString;
use std::path::PathBuf;

fn run(args: &[&str]) -> Result<Command, String> {
    parse(args.iter().map(OsString::from)).map_err(|error| error.to_string())
}

fn source(path: &str, password: &[u8], recovery: Recovery) -> Source {
    Source {
        path: PathBuf::from(path),
        password: password.to_vec(),
        recovery,
    }
}

#[test]
fn help_wins_anywhere_and_version_stands_alone() {
    assert_eq!(run(&["--help"]), Ok(Command::Help));
    assert_eq!(run(&["render", "-h", "--bogus"]), Ok(Command::Help));
    assert_eq!(run(&["--version"]), Ok(Command::Version));
    // A harness puts rendering options before every invocation.
    for args in [
        &["--accuracy", "competitive", "--version"][..],
        &["--fast", "byte-storage", "--version"],
        &["--overprint", "never", "--version"],
    ] {
        assert_eq!(run(args), Ok(Command::Version), "{args:?}");
    }
    assert_eq!(
        run(&["--version", "--strict"]),
        Err("--strict does not apply to --version".into())
    );
    assert_eq!(
        run(&["inspect", "--version", "a.pdf"]),
        Err("--version does not apply to inspect".into())
    );
}

#[test]
fn options_may_appear_anywhere_but_only_where_they_apply() {
    assert_eq!(
        run(&["--strict", "inspect", "a.pdf", "--password", "pw"]),
        Ok(Command::Inspect(source("a.pdf", b"pw", Recovery::Strict)))
    );
    assert_eq!(
        run(&["inspect", "a.pdf"]),
        Ok(Command::Inspect(source("a.pdf", b"", Recovery::Repair)))
    );
    for (args, flag, command) in [
        (
            &["inspect", "--timings", "a.pdf"][..],
            "--timings",
            "inspect",
        ),
        (&["scan", "--strict", "a.pdf"], "--strict", "scan"),
        (
            &["roundtrip", "--strict", "a.pdf", "b.pdf"],
            "--strict",
            "roundtrip",
        ),
        (
            &["text", "--max-pages", "2", "a.pdf"],
            "--max-pages",
            "text",
        ),
        (
            &["microbench", "--timings", "a.pdf", "open-pages", "1"],
            "--timings",
            "microbench",
        ),
        (&["licenses", "--password", "x"], "--password", "licenses"),
        (
            &["inspect", "--overprint", "never", "a.pdf"],
            "--overprint",
            "inspect",
        ),
    ] {
        assert_eq!(
            run(args),
            Err(format!("{flag} does not apply to {command}")),
            "{args:?}"
        );
    }
}

#[test]
fn malformed_arguments_name_what_is_wrong() {
    for (args, message) in [
        (&[][..], "missing command"),
        (&["frobnicate"], "unknown command frobnicate"),
        (&["--bogus", "inspect", "a.pdf"], "unknown option --bogus"),
        (&["inspect"], "wrong number of arguments for inspect"),
        (
            &["inspect", "a.pdf", "b.pdf"],
            "wrong number of arguments for inspect",
        ),
        (
            &["inspect", "a.pdf", "--password"],
            "--password needs a value",
        ),
        (
            &["text", "a.pdf", "x"],
            "PAGE must be a non-negative integer, not x",
        ),
        (
            &["text", "a.pdf", "-1"],
            "PAGE must be a non-negative integer, not -1",
        ),
        (
            &["render", "a.pdf", "b.ppm", "nan"],
            "DPI must be a positive number, not nan",
        ),
        (
            &["render", "a.pdf", "b.ppm", "-72"],
            "DPI must be a positive number, not -72",
        ),
        (
            &["render", "a.pdf", "b.ppm", "0"],
            "DPI must be a positive number, not 0",
        ),
        (
            &["convert", "a.pdf", "out", "72", "1"],
            "wrong number of arguments for convert",
        ),
        (
            &["convert", "a.pdf", "out", "72", "1", "z"],
            "LAST must be a non-negative integer, not z",
        ),
        (
            &["scan", "--max-pages", "many", "a.pdf"],
            "--max-pages must be a non-negative integer, not many",
        ),
        (
            &["microbench", "a.pdf", "warm", "1"],
            "OPERATION must be open-pages, render-cold, record-cold, record-warm or render-warm",
        ),
        (
            &["microbench", "a.pdf", "open-pages", "0"],
            "COUNT must be 1 to 100000",
        ),
        (
            &["microbench", "a.pdf", "open-pages", "100001"],
            "COUNT must be 1 to 100000",
        ),
    ] {
        assert_eq!(run(args), Err(message.into()), "{args:?}");
    }
}

#[test]
fn conversions_select_pages_dpi_and_timings() {
    assert_eq!(
        run(&["render", "--timings", "a.pdf", "b.ppm", "144", "3"]),
        Ok(Command::Convert(Conversion {
            source: source("a.pdf", b"", Recovery::Repair),
            output: Output::File(PathBuf::from("b.ppm")),
            render: RenderOptions::from_dpi(144.0),
            pages: Some(3..=3),
            timings: Timings::Print,
        }))
    );
    assert_eq!(
        run(&["convert", "a.pdf", "out"]),
        Ok(Command::Convert(Conversion {
            source: source("a.pdf", b"", Recovery::Repair),
            output: Output::Directory(PathBuf::from("out")),
            render: RenderOptions::from_dpi(72.0),
            pages: None,
            timings: Timings::Off,
        }))
    );
    let Ok(Command::Convert(conversion)) = run(&["convert", "a.pdf", "out", "36.5", "1", "4"])
    else {
        panic!("convert with a range");
    };
    assert_eq!(
        (conversion.render, conversion.pages),
        (RenderOptions::from_dpi(36.5), Some(1..=4))
    );
}

#[test]
fn the_other_commands_parse_their_operands() {
    assert_eq!(run(&["licenses"]), Ok(Command::Licenses));
    assert_eq!(
        run(&["text", "a.pdf", "2"]),
        Ok(Command::Text {
            source: source("a.pdf", b"", Recovery::Repair),
            page: Some(2),
        })
    );
    assert_eq!(
        run(&["find", "--strict", "a.pdf", "two words"]),
        Ok(Command::Find {
            source: source("a.pdf", b"", Recovery::Strict),
            query: "two words".into(),
        })
    );
    assert_eq!(
        run(&["scan", "a.pdf", "--max-pages", "5", "--password", "p"]),
        Ok(Command::Scan {
            source: source("a.pdf", b"p", Recovery::Strict),
            render: RenderOptions::from_dpi(72.0),
            max_pages: Some(5),
        })
    );
    assert_eq!(
        run(&["roundtrip", "a.pdf", "b.pdf"]),
        Ok(Command::Roundtrip {
            source: source("a.pdf", b"", Recovery::Strict),
            output: PathBuf::from("b.pdf"),
        })
    );
    let operation = |name| match run(&["microbench", "a.pdf", name, "1"]) {
        Ok(Command::Microbench { operation, .. }) => operation,
        other => panic!("{name}: {other:?}"),
    };
    assert_eq!(operation("render-cold"), Bench::RenderCold);
    assert_eq!(operation("record-cold"), Bench::RecordCold);
    assert_eq!(operation("record-warm"), Bench::RecordWarm);
    assert_eq!(
        run(&["microbench", "--strict", "a.pdf", "render-warm", "10", "96"]),
        Ok(Command::Microbench {
            source: source("a.pdf", b"", Recovery::Strict),
            operation: Bench::RenderWarm,
            count: 10,
            render: RenderOptions::from_dpi(96.0),
        })
    );
}

#[cfg(unix)]
#[test]
fn paths_need_not_be_utf8_but_text_must_be() {
    use std::os::unix::ffi::OsStringExt;
    let path = OsString::from_vec(b"in\xff.pdf".to_vec());
    let parsed = parse([OsString::from("inspect"), path.clone()]);
    assert_eq!(
        parsed,
        Ok(Command::Inspect(Source {
            path: PathBuf::from(path.clone()),
            password: Vec::new(),
            recovery: Recovery::Repair,
        }))
    );
    let parsed = parse([OsString::from("find"), OsString::from("a.pdf"), path]);
    assert_eq!(
        parsed.map_err(|error| error.to_string()),
        Err("QUERY must be UTF-8 text".into())
    );
}

#[test]
fn overprint_selects_the_preview_of_every_rendering_command() {
    let preview = |args: &[&str]| match run(args) {
        Ok(Command::Convert(conversion)) => conversion.render.overprint,
        Ok(Command::Scan { render, .. } | Command::Microbench { render, .. }) => render.overprint,
        other => panic!("{args:?}: {other:?}"),
    };
    assert_eq!(
        preview(&["render", "a.pdf", "b.ppm"]),
        OverprintPreview::Auto
    );
    for (mode, expected) in [
        ("auto", OverprintPreview::Auto),
        ("always", OverprintPreview::Always),
        ("never", OverprintPreview::Never),
    ] {
        for args in [
            &["render", "a.pdf", "b.ppm"][..],
            &["convert", "a.pdf", "out"],
            &["scan", "a.pdf"],
            &["microbench", "a.pdf", "render-cold", "1"],
        ] {
            let mut args = args.to_vec();
            args.extend(["--overprint", mode]);
            assert_eq!(preview(&args), expected, "{args:?}");
        }
    }
}

#[test]
fn accuracy_selects_the_presets_of_every_rendering_command() {
    let fast = |args: &[&str]| match run(args) {
        Ok(Command::Convert(conversion)) => conversion.render.fast,
        Ok(Command::Scan { render, .. } | Command::Microbench { render, .. }) => render.fast,
        other => panic!("{args:?}: {other:?}"),
    };
    assert_eq!(fast(&["render", "a.pdf", "b.ppm"]), FastParams::EXACT);
    for (mode, expected) in [
        ("exact", FastParams::EXACT),
        ("competitive", FastParams::COMPETITIVE),
    ] {
        for args in [
            &["render", "a.pdf", "b.ppm"][..],
            &["convert", "a.pdf", "out"],
            &["scan", "a.pdf"],
            &["microbench", "a.pdf", "render-cold", "1"],
        ] {
            let mut args = args.to_vec();
            args.extend(["--accuracy", mode]);
            assert_eq!(fast(&args), expected, "{args:?}");
        }
    }
    assert_eq!(
        run(&["render", "--accuracy", "fast", "a.pdf", "b.ppm"]),
        Err("--accuracy must be exact or competitive".into())
    );
    assert_eq!(
        run(&["inspect", "--accuracy", "exact", "a.pdf"]),
        Err("--accuracy does not apply to inspect".into())
    );
}

#[test]
fn fast_switches_each_parameter_by_name() {
    let fast = |list: &str| match run(&["convert", "--fast", list, "a.pdf", "out"]) {
        Ok(Command::Convert(conversion)) => Ok(conversion.render.fast),
        Ok(other) => panic!("{list}: {other:?}"),
        Err(error) => Err(error),
    };
    assert_eq!(fast("none"), Ok(FastParams::EXACT));
    assert_eq!(
        fast("byte-storage"),
        Ok(FastParams {
            byte_storage: true,
            ..FastParams::EXACT
        })
    );
    assert_eq!(
        fast("glyph-bins,reduced-image-decode"),
        Ok(FastParams {
            glyph_origin_bins: true,
            reduced_image_decode: true,
            ..FastParams::EXACT
        })
    );
    assert_eq!(
        fast("glyph-bins,approximate-coverage,byte-storage,reduced-image-decode"),
        Ok(FastParams::COMPETITIVE)
    );
    let names =
        "--fast takes glyph-bins, approximate-coverage, byte-storage, reduced-image-decode or none";
    for wrong in [
        "",
        "fast",
        "none,byte-storage",
        "byte-storage,",
        "Glyph-Bins",
    ] {
        assert_eq!(fast(wrong), Err(names.into()), "{wrong:?}");
    }
    for args in [
        &[
            "render",
            "--fast",
            "none",
            "--accuracy",
            "exact",
            "a.pdf",
            "b.ppm",
        ][..],
        &[
            "render",
            "--accuracy",
            "exact",
            "--fast",
            "none",
            "a.pdf",
            "b.ppm",
        ],
    ] {
        assert_eq!(
            run(args),
            Err("--accuracy and --fast are alternatives".into()),
            "{args:?}"
        );
    }
    assert_eq!(
        run(&["text", "--fast", "none", "a.pdf"]),
        Err("--fast does not apply to text".into())
    );
}
