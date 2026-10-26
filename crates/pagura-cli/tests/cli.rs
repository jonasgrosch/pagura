#![forbid(unsafe_code)]
//! The command-line tool, run as a subprocess.

use pagura_testkit::{Json, Manifest, assert_bytes_eq, fixture, fixture_path};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A temporary directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pagura-cli-{}-{id}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The first classic case expected to be unsupported, as (fixture path,
/// feature tag).
#[expect(
    clippy::expect_used,
    reason = "generated unsupported cases name their fixture and feature"
)]
fn unsupported_case() -> (PathBuf, String) {
    let manifest = Manifest::load("classic");
    let case = manifest
        .cases()
        .find(|case| case.string("expected") == Some("unsupported"))
        .expect("the classic manifest has an unsupported case");
    let feature = case
        .string("feature")
        .expect("unsupported cases name a feature");
    (case.path().expect("a fixture path"), feature.to_owned())
}

fn pagura() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pagura"))
}

fn service_report(
    command: &str,
    area: &str,
    file: &str,
) -> Result<Json, Box<dyn std::error::Error>> {
    let output = pagura()
        .arg(command)
        .arg(fixture_path(area, file))
        .output()?;
    if !output.status.success() {
        return Err(
            std::io::Error::other(String::from_utf8_lossy(&output.stderr).into_owned()).into(),
        );
    }
    Ok(Json::parse(&String::from_utf8(output.stdout)?)?)
}

#[test]
fn metadata_json_keeps_unicode_fields_and_stream_identity_separate() {
    let report = service_report("metadata", "metadata", "populated-flate.pdf").unwrap();
    assert_eq!(report.get("schema").and_then(Json::as_u64), Some(1));
    assert_eq!(report.get("language").and_then(Json::as_str), Some("de-DE"));
    let info = report.get("info").unwrap();
    for (key, value) in [
        ("Title", "Grüße Ω"),
        ("Author", "Euro €"),
        ("Subject", "測試"),
        ("Trapped", "True"),
        ("CreationDate", "D:20261026030000+01'00"),
    ] {
        assert_eq!(info.get(key).and_then(Json::as_str), Some(value), "{key}");
    }
    assert_eq!(
        info.get("object")
            .unwrap()
            .get("number")
            .and_then(Json::as_u64),
        Some(5)
    );
    assert_eq!(
        report
            .get("metadata_object")
            .unwrap()
            .get("number")
            .and_then(Json::as_u64),
        Some(6)
    );
    let absent = service_report("metadata", "metadata", "absent.pdf").unwrap();
    for key in ["info", "language", "metadata_object"] {
        assert!(absent.get(key).unwrap().is_null(), "{key}");
    }
    let empty = service_report("metadata", "metadata", "empty.pdf").unwrap();
    assert!(empty.get("info").unwrap().get("Title").unwrap().is_null());
    assert_eq!(
        empty
            .get("info")
            .unwrap()
            .get("Trapped")
            .and_then(Json::as_str),
        Some("Unknown")
    );
}

#[test]
fn attachment_listing_preserves_filename_bytes_unicode_and_payload_choices() {
    let report = service_report("attachments", "files", "populated.pdf").unwrap();
    assert_eq!(report.get("schema").and_then(Json::as_u64), Some(1));
    let files = report.get("files").and_then(Json::as_array).unwrap();
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!(file.get("index").and_then(Json::as_u64), Some(0));
    assert_eq!(
        file.get("key_hex").and_then(Json::as_str),
        Some("6c6f6f6b75702d6b6579")
    );
    let names = file.get("names").unwrap();
    assert_eq!(names.get("UF").and_then(Json::as_str), Some("Grüße Ω.txt"));
    assert_eq!(
        names.get("F_hex").and_then(Json::as_str),
        Some("6c65676163792e747874")
    );
    assert!(names.get("DOS_hex").unwrap().is_null());
    assert_eq!(
        file.get("description").and_then(Json::as_str),
        Some("Attachment €")
    );
    assert_eq!(
        file.get("relationship").and_then(Json::as_str),
        Some("Data")
    );
    assert_eq!(file.get("volatile").and_then(Json::as_bool), Some(true));
    let payloads = file.get("payloads").and_then(Json::as_array).unwrap();
    assert_eq!(payloads.len(), 2);
    for (payload, key) in payloads.iter().zip(["F", "UF"]) {
        assert_eq!(payload.get("key").and_then(Json::as_str), Some(key));
        assert_eq!(
            payload.get("declared_size").and_then(Json::as_u64),
            Some(18)
        );
        assert_eq!(
            payload
                .get("object")
                .unwrap()
                .get("number")
                .and_then(Json::as_u64),
            Some(6)
        );
    }
    let raw = service_report("attachments", "files", "simple-string.pdf").unwrap();
    let file = &raw.get("files").and_then(Json::as_array).unwrap()[0];
    assert_eq!(
        file.get("names")
            .unwrap()
            .get("F_hex")
            .and_then(Json::as_str),
        Some("2f726177ff2f66696c65")
    );
    assert!(
        file.get("payloads")
            .and_then(Json::as_array)
            .unwrap()
            .is_empty()
    );
    for name in ["absent.pdf", "null-tree.pdf", "empty-tree.pdf"] {
        let report = service_report("attachments", "files", name).unwrap();
        assert!(
            report
                .get("files")
                .and_then(Json::as_array)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn service_reports_reject_bad_types_and_preserve_strict_repairs() {
    for (command, area, file) in [
        ("metadata", "metadata", "info-field-wrong-type.pdf"),
        ("metadata", "metadata", "metadata-cycle.pdf"),
        ("attachments", "files", "embedded-nonstream.pdf"),
        ("attachments", "files", "spec-cycle.pdf"),
    ] {
        let output = pagura()
            .arg(command)
            .arg(fixture_path(area, file))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{file}");
        assert!(output.stdout.is_empty(), "{file}");
    }
    for (command, area, file) in [
        ("metadata", "metadata", "invalid-text.pdf"),
        ("attachments", "files", "invalid-unicode.pdf"),
    ] {
        let repaired = service_report(command, area, file).unwrap();
        assert!(
            !repaired
                .get("repairs")
                .and_then(Json::as_array)
                .unwrap()
                .is_empty()
        );
        let output = pagura()
            .args([command, "--strict"])
            .arg(fixture_path(area, file))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{file}");
        assert!(output.stdout.is_empty(), "{file}");
    }
}

#[test]
fn extraction_writes_decoded_bytes_only_to_the_requested_new_file() {
    let scratch = Scratch::new().unwrap();
    for (command, area, name, args, expected) in [
        ("extract-metadata", "metadata", "populated-flate.pdf", &[][..], b"<?xpacket begin=\"\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><title>XMP title</title></x:xmpmeta><?xpacket end=\"w\"?>".as_slice()),
        ("extract-metadata", "metadata", "empty.pdf", &[], b""),
        ("extract-metadata", "metadata", "metadata-inert-xml.pdf", &[], b"<!DOCTYPE x [<!ENTITY e SYSTEM \"https://example.invalid/metadata\">]><x>&e;</x>"),
        ("extract-attachment", "files", "flate.pdf", &["0", "UF"], b"attachment\0bytes\xff\n"),
        ("extract-attachment", "files", "empty-payload.pdf", &["0", "F"], b""),
    ] {
        let target = scratch.0.join(name);
        let output = pagura().arg(command).arg(fixture_path(area, name)).args(args).arg(&target).output().unwrap();
        assert!(output.status.success(), "{name}: {:?}", output.stderr);
        assert!(output.stdout.is_empty());
        assert_eq!(std::fs::read(&target).unwrap(), expected);
        let repeated = pagura().arg(command).arg(fixture_path(area, name)).args(args).arg(&target).output().unwrap();
        assert_eq!(repeated.status.code(), Some(1));
        assert_eq!(std::fs::read(&target).unwrap(), expected);
    }
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 5);
}

#[test]
fn failed_extraction_leaves_no_output_and_passwords_authorize_payload_reads() {
    let scratch = Scratch::new().unwrap();
    let target = scratch.0.join("output");
    for (command, area, file, args) in [
        ("extract-metadata", "metadata", "absent.pdf", &[][..]),
        ("extract-attachment", "files", "populated.pdf", &["1", "F"]),
        (
            "extract-attachment",
            "files",
            "populated.pdf",
            &["0", "Unix"],
        ),
        (
            "extract-attachment",
            "files",
            "populated.pdf",
            &["0", "invalid-key"],
        ),
        (
            "extract-attachment",
            "files",
            "simple-string.pdf",
            &["0", "F"],
        ),
        (
            "extract-attachment",
            "files",
            "external-stream.pdf",
            &["0", "F"],
        ),
    ] {
        let output = pagura()
            .arg(command)
            .arg(fixture_path(area, file))
            .args(args)
            .arg(&target)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{file}: {args:?}");
        assert!(!target.exists());
    }
    for password in ["", "wrong", "attachment", "owner"] {
        let output = pagura()
            .args(["extract-attachment", "--password", password])
            .arg(fixture_path("files", "encrypted-no-type.pdf"))
            .args(["0", "F"])
            .arg(&target)
            .output()
            .unwrap();
        if matches!(password, "attachment" | "owner") {
            assert!(output.status.success(), "{password}: {:?}", output.stderr);
            assert_eq!(std::fs::read(&target).unwrap(), b"attached file bytes");
            std::fs::remove_file(&target).unwrap();
        } else {
            assert_eq!(output.status.code(), Some(1));
            assert!(!target.exists());
        }
    }
}

#[test]
fn renders_exact_pixels_and_roundtrip_preserves_original_bytes() {
    let scratch = Scratch::new().unwrap();
    let pdf = fixture_path("classic", "rectangle.pdf");
    let image = scratch.0.join("render.ppm");
    let saved = scratch.0.join("saved.pdf");
    assert!(
        pagura()
            .arg("render")
            .arg(&pdf)
            .arg(&image)
            .status()
            .unwrap()
            .success()
    );
    assert_bytes_eq(
        &std::fs::read(image).unwrap(),
        &fixture("classic", "rectangle.ppm"),
        "CLI render",
    );
    assert!(
        pagura()
            .arg("roundtrip")
            .arg(&pdf)
            .arg(&saved)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(std::fs::read(saved).unwrap(), std::fs::read(pdf).unwrap());
}

#[test]
fn inspect_reports_repairs_and_strict_mode_rejects_them() {
    let pdf = fixture_path("classic", "trailing-bytes.pdf");
    let output = pagura().arg("inspect").arg(&pdf).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("\"repairs\":[\"file.trailing-bytes\"]"),
        "{stdout}"
    );
    let strict = pagura()
        .args(["inspect", "--strict"])
        .arg(&pdf)
        .output()
        .unwrap();
    assert!(!strict.status.success());
}

#[test]
fn does_not_overwrite_files_or_emit_partial_unsupported_images() {
    let scratch = Scratch::new().unwrap();
    let existing = scratch.0.join("existing.ppm");
    std::fs::write(&existing, b"preserve me").unwrap();
    let result = pagura()
        .arg("render")
        .arg(fixture_path("classic", "rectangle.pdf"))
        .arg(&existing)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read(existing).unwrap(), b"preserve me");
    let (unsupported, feature) = unsupported_case();
    let missing = scratch.0.join("unsupported.ppm");
    let result = pagura()
        .arg("render")
        .arg(unsupported)
        .arg(&missing)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains(&format!("unsupported [{feature}]")));
    assert!(!missing.exists());
    let strict = scratch.0.join("strict.ppm");
    let result = pagura()
        .args(["render", "--strict"])
        .arg(fixture_path("paint", "image-fax-grid-crop.pdf"))
        .arg(&strict)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("image.ccitt-columns"));
    assert!(
        !strict.exists(),
        "preparation repair created a file under strict recovery"
    );
}

#[test]
fn streamed_cli_ppms_equal_collected_public_api_output() {
    let scratch = Scratch::new().unwrap();
    for (area, name) in [
        ("classic", "rectangle"),
        ("classic", "many-rectangles"),
        ("paint", "image-page-rgb"),
        ("transparency", "gray-blend-Normal"),
        ("transparency", "gray-blend-Multiply"),
        ("overprint", "preview-pdfx-intent"),
    ] {
        let input = fixture_path(area, &format!("{name}.pdf"));
        let image = scratch.0.join(format!("{area}-{name}.ppm"));
        let result = pagura()
            .arg("render")
            .arg(&input)
            .arg(&image)
            .arg("150")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{area}/{name}: {:?}",
            result.stderr
        );
        let document = pagura::Document::open(
            std::fs::read(input).unwrap(),
            &pagura::OpenOptions::default(),
        )
        .unwrap();
        let context = pagura::render_context(std::sync::Arc::new(document));
        let pixmap = context
            .render_page(
                context.document().page(0).unwrap(),
                &pagura::RenderOptions::from_dpi(150.0),
            )
            .unwrap();
        assert!(
            pixmap
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
        let expected = pagura_testkit::ppm(
            pixmap.width(),
            pixmap.height(),
            &pixmap.to_rgb_over(pagura::Rgba8::WHITE),
        );
        assert_bytes_eq(&std::fs::read(image).unwrap(), &expected, name);
    }
}

#[test]
fn convert_writes_every_page_and_reports_stage_timings() {
    let scratch = Scratch::new().unwrap();
    let output = pagura()
        .args(["convert", "--timings"])
        .arg(fixture_path("classic", "graphics-state.pdf"))
        .arg(&scratch.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let image = std::fs::read(scratch.0.join("page-0001.ppm")).unwrap();
    assert_bytes_eq(
        &image,
        &fixture("classic", "graphics-state.ppm"),
        "converted page",
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    for field in [
        "\"open_ns\":",
        "\"render_ns\":",
        "\"emit_ns\":",
        "\"total_ns\":",
    ] {
        assert!(stdout.contains(field), "{field} missing from {stdout}");
    }
}

#[test]
fn convert_page_ranges_are_validated_before_writing() {
    let scratch = Scratch::new().unwrap();
    let pdf = fixture_path("classic", "two-pages.pdf");
    let status = pagura()
        .arg("convert")
        .arg(&pdf)
        .arg(&scratch.0)
        .args(["72", "1", "1"])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!scratch.0.join("page-0001.ppm").exists());
    assert!(scratch.0.join("page-0002.ppm").exists());
    let empty = Scratch::new().unwrap();
    let status = pagura()
        .arg("convert")
        .arg(&pdf)
        .arg(&empty.0)
        .args(["72", "1", "2"])
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(std::fs::read_dir(&empty.0).unwrap().count(), 0);
}

#[test]
fn licenses_reproduces_every_linked_data_notice() {
    let output = pagura().arg("licenses").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    for (subject, notice) in pagura::notices() {
        assert!(text.contains(subject));
        assert!(text.contains(notice));
    }
    assert_eq!(
        text.contains("No bundled data is linked."),
        !pagura::provider::BUNDLED
    );
}

#[test]
fn scan_retries_a_late_repair_without_losing_earlier_pages() {
    let input = fixture_path("paint", "text-late-font-repair.pdf");
    let all = pagura().arg("scan").arg(&input).output().unwrap();
    assert!(all.status.success());
    let text = String::from_utf8(all.stdout).unwrap();
    assert!(text.contains("\"strict\":false"), "{text}");
    assert!(text.contains("\"rendered\":2"), "{text}");
    assert!(
        text.contains("\"repairs\":[\"font.stream-lengths\"]"),
        "{text}"
    );
    assert!(text.contains("\"failures\":[]"), "{text}");
    let first = pagura()
        .args(["scan", "--max-pages", "1"])
        .arg(input)
        .output()
        .unwrap();
    assert!(first.status.success());
    let text = String::from_utf8(first.stdout).unwrap();
    assert!(text.contains("\"strict\":true"), "{text}");
    assert!(text.contains("\"rendered\":1"), "{text}");
    assert!(text.contains("\"repairs\":[]"), "{text}");
}

#[test]
#[cfg(feature = "bundled-data")]
fn scan_separates_information_from_repairs() {
    for (name, strict, repairs) in [
        ("bundled-helvetica", true, "[]"),
        (
            "bundled-diagnostic-kinds",
            false,
            "[\"font.descriptor-weight\"]",
        ),
    ] {
        let output = pagura()
            .arg("scan")
            .arg(fixture_path("paint", &format!("{name}.pdf")))
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&format!("\"strict\":{strict}")), "{text}");
        assert!(text.contains(&format!("\"repairs\":{repairs}")), "{text}");
        assert!(
            text.contains("\"information\":[\"font.substituted\"]"),
            "{text}"
        );
        assert!(text.contains("\"failures\":[]"), "{text}");
    }
}

#[test]
fn overprint_option_selects_the_preview_and_validates_its_value() {
    let scratch = Scratch::new().unwrap();
    for input in ["preview-pdfx-intent", "preview-no-intent"] {
        for mode in ["auto", "always", "never"] {
            let image = scratch.0.join(format!("{input}-{mode}.ppm"));
            let output = pagura()
                .args(["render", "--overprint", mode])
                .arg(fixture_path("overprint", &format!("{input}.pdf")))
                .arg(&image)
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output.stderr);
            let simulate = mode == "always" || (mode == "auto" && input == "preview-pdfx-intent");
            let oracle = if simulate {
                "preview-pdfx-intent"
            } else {
                "preview-no-intent"
            };
            assert_bytes_eq(
                &std::fs::read(image).unwrap(),
                &fixture("overprint", &format!("{oracle}.ppm")),
                mode,
            );
        }
    }
    for mode in [Some("invalid"), None] {
        let mut command = pagura();
        command.args(["render", "--overprint"]);
        if let Some(mode) = mode {
            command.arg(mode);
        }
        assert!(!command.output().unwrap().status.success());
    }
    // Scan must honor the policy too, including simulated resource limits.
    let scan = pagura()
        .args(["scan", "--overprint", "never"])
        .arg(fixture_path("overprint", "spot-budget.pdf"))
        .output()
        .unwrap();
    assert!(scan.status.success());
    assert!(
        String::from_utf8(scan.stdout)
            .unwrap()
            .contains("\"failures\":[]")
    );
}

#[test]
fn cli_copy_and_find_use_zero_based_pages_and_default_user_space() {
    let path = fixture_path("text", "extract-tj.pdf");
    let output = pagura().arg("text").arg(&path).arg("0").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"ABCD EF\n");
    let output = pagura()
        .arg("find")
        .arg(path)
        .arg("abcd ef")
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.starts_with("{\"page\":0,\"start\":0,\"end\":7,\"quads\":[[[10,60]"),
        "{output}"
    );
}

#[test]
fn help_succeeds_and_usage_errors_exit_with_status_two() {
    let help = pagura().arg("--help").output().unwrap();
    assert_eq!(help.status.code(), Some(0));
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.starts_with("usage: pagura inspect"), "{text}");
    let pdf = fixture_path("classic", "rectangle.pdf");
    for args in [
        &["inspect", "--timings"][..],
        &["scan", "--strict"],
        &["render", "--bogus"],
        &["render"],
    ] {
        let output = pagura().args(args).arg(&pdf).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("\nusage: pagura"), "{stderr}");
    }
    let missing = pagura()
        .arg("inspect")
        .arg(fixture_path("classic", "no-such-file.pdf"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
}

/// A harness puts rendering options before every invocation, `--version`
/// included.
#[test]
fn version_accepts_rendering_options_before_it() {
    for args in [&["--version"][..], &["--overprint", "never", "--version"]] {
        let output = pagura().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with("pagura "), "{args:?}: {text}");
    }
    let strict = pagura().args(["--strict", "--version"]).output().unwrap();
    assert_eq!(strict.status.code(), Some(2));
}

#[test]
fn scan_labels_unreadable_input_as_io() {
    let output = pagura()
        .arg("scan")
        .arg(fixture_path("classic", "no-such-file.pdf"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.starts_with("{\"schema\":1,\"strict\":false,\"open\":{\"kind\":\"io\""),
        "{text}"
    );
}
