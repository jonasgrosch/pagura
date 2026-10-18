#![forbid(unsafe_code)]
//! The command-line tool, run as a subprocess.

mod support;

use pagura_testkit::{assert_bytes_eq, fixture, fixture_path};
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

fn pagura() -> Command {
    Command::new(env!("CARGO_BIN_EXE_pagura"))
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
    let (unsupported, feature) = support::unsupported_cases().remove(0);
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
    let mut count = 0;
    assert!(text.contains(pagura_crypto::unicode_notice()));
    for (subject, notice) in pagura::data_notices() {
        assert!(text.contains(subject));
        assert!(text.contains(notice));
        count += 1;
    }
    assert_eq!(text.contains("No bundled data is linked."), count == 0);
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
