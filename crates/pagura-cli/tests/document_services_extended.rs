#![forbid(unsafe_code)]
//! Public executable source-service tests; byte expectations are independently authored.
use pagura_testkit::{Json, fixture_path};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Directory(PathBuf);
impl Directory {
    fn new() -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "pagura-services-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn run(
    command: &str,
    options: &[&str],
    name: &str,
    operands: &[&std::ffi::OsStr],
) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_pagura"))
        .arg(command)
        .args(options)
        .arg(fixture_path("document_services", name))
        .args(operands)
        .output()
}
fn report(output: &Output) -> Result<Json> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(Json::parse(std::str::from_utf8(&output.stdout)?)?)
}

#[test]
fn scoped_listing_and_related_payloads_preserve_exact_selection() -> Result<()> {
    for options in [
        vec!["--associated"],
        vec!["--associated", "--page", "0"],
        vec!["--object", "10:0"],
    ] {
        let output = run("attachments", &options, "complete.pdf", &[])?;
        let json = report(&output)?;
        assert_eq!(json.get("schema").and_then(Json::as_u64), Some(2));
        assert!(json.get("repairs").is_some());
        assert!(json.get("information").is_some());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("checksum_hex"));
        assert!(text.contains("identifiers_hex"));
        assert!(text.contains("9007199254740993"));
        assert!(
            text.contains(
                "same-name"
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
                    .as_str()
            )
        );
    }
    let directory = Directory::new()?;
    let path = directory.0.join("related.bin");
    let output = run(
        "extract-attachment",
        &["--associated", "--page", "0", "--related", "1"],
        "complete.pdf",
        &["1".as_ref(), "F".as_ref(), path.as_os_str()],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"second sidecar\0");
    let again = run(
        "extract-attachment",
        &["--associated", "--related", "0"],
        "complete.pdf",
        &["0".as_ref(), "F".as_ref(), path.as_os_str()],
    )?;
    assert!(!again.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), b"second sidecar\0");
    let missing = directory.0.join("absent.bin");
    let output = run(
        "extract-attachment",
        &["--related", "2"],
        "complete.pdf",
        &["0".as_ref(), "F".as_ref(), missing.as_os_str()],
    )?;
    assert!(!output.status.success());
    assert!(!missing.exists());
    Ok(())
}

#[test]
fn collection_metadata_and_image_exports_use_explicit_paths() -> Result<()> {
    let directory = Directory::new()?;
    let packet = directory.0.join("packet.bin");
    let output = run(
        "extract-metadata",
        &["--object", "10:0"],
        "complete.pdf",
        &[packet.as_os_str()],
    )?;
    assert!(output.status.success());
    assert_eq!(
        std::fs::read(&packet).unwrap(),
        b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">object packet</x:xmpmeta>"
    );
    let collection = run("collection", &[], "complete.pdf", &[])?;
    let value = report(&collection)?;
    assert_eq!(
        value
            .get("collection")
            .unwrap()
            .get("view")
            .and_then(Json::as_str),
        Some("Hidden")
    );
    let images = run("images", &["--page", "0"], "complete.pdf", &[])?;
    report(&images)?;
    assert!(String::from_utf8_lossy(&images.stdout).contains("direct-resources-or-object"));
    let image = directory.0.join("image.bin");
    let output = run(
        "extract-image",
        &["--object", "9:0"],
        "complete.pdf",
        &[image.as_os_str()],
    )?;
    assert!(output.status.success());
    assert_eq!(std::fs::read(&image).unwrap(), [255, 0, 0, 0, 255, 0]);
    let thumbnail = directory.0.join("thumb.bin");
    let output = run(
        "extract-thumbnail",
        &["--page", "0"],
        "complete.pdf",
        &[thumbnail.as_os_str()],
    )?;
    assert!(output.status.success());
    assert_eq!(std::fs::read(thumbnail).unwrap(), [127]);
    Ok(())
}

#[test]
fn malformed_sources_and_bad_options_do_not_emit_complete_reports() -> Result<()> {
    for (command, options, name) in [
        ("collection", vec![], "invalid-sort.pdf"),
        ("attachments", vec!["--associated"], "invalid-item.pdf"),
        ("images", vec!["--object", "9:0"], "invalid-image-width.pdf"),
    ] {
        let output = run(command, &options, name, &[])?;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }
    for options in [
        vec!["--object", "10"],
        vec!["--object", "10:65536"],
        vec!["--object", "10:0", "--page", "0"],
        vec!["--related", "0"],
    ] {
        let output = run("attachments", &options, "complete.pdf", &[])?;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    Ok(())
}
