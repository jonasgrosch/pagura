#![forbid(unsafe_code)]
//! Inspects navigation data without activating targets.
//! Usage: cargo run --release -p pagura-document --example navigation -- FILE
use pagura_core::{Error, Value};
use pagura_document::source::ReadResult;
use pagura_document::{Document, OpenOptions};
use std::sync::Arc;

#[derive(Default)]
struct Stats {
    items: usize,
    repairs: usize,
    errors: usize,
    present: bool,
}
fn inspect(bytes: Arc<[u8]>, kind: &str) -> ReadResult<Stats> {
    // Each view gets a fresh document so diagnostic deduplication across views
    // cannot hide that both required the same repair. Source bytes are shared.
    let doc = Document::open(bytes, &OpenOptions::default())?;
    let before = doc.diagnostics().len();
    let mut stats = Stats::default();
    let catalog = doc.catalog()?;
    let result = match kind {
        "outlines" => {
            stats.present = catalog.get(b"Outlines").is_some_and(|v| !v.is_null());
            doc.outlines().map(|v| v.len())
        }
        "destinations" => {
            stats.present = catalog.contains_key(b"Dests")
                || catalog
                    .get(b"Names")
                    .and_then(|v| doc.resolve_dictionary(v).ok())
                    .is_some_and(|n| n.contains_key(b"Dests"));
            doc.named_destinations().map(|v| v.len())
        }
        "links" => {
            for page in doc.pages()? {
                if let Some(annots) = page
                    .dictionary()
                    .get(b"Annots")
                    .and_then(|v| doc.resolve(v).ok())
                    .and_then(Value::as_array)
                {
                    stats.present |= annots.iter().any(|v| {
                        doc.resolve_dictionary(v)
                            .is_ok_and(|d| d.is_name(b"Subtype", b"Link"))
                    });
                }
                match page.links(&doc) {
                    Ok(links) => stats.items += links.len(),
                    Err(error) => {
                        stats.errors += 1;
                        eprintln!(
                            "links\t{}\t{}",
                            error
                                .as_error()
                                .map_or("unavailable", |error| error.kind.name()),
                            error
                        );
                    }
                }
            }
            Ok(stats.items)
        }
        _ => Err(Error::syntax("unknown navigation view").into()),
    };
    match result {
        Ok(items) => {
            stats.items = items;
            if kind != "links" {
                stats.present = items > 0;
            }
        }
        Err(error) => {
            stats.errors += 1;
            eprintln!(
                "{kind}\t{}\t{}",
                error
                    .as_error()
                    .map_or("unavailable", |error| error.kind.name()),
                error
            );
        }
    }
    stats.repairs = doc
        .diagnostics()
        .iter()
        .skip(before)
        .filter(|item| item.repair().is_some())
        .count();
    Ok(stats)
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("expected an input PDF path")?;
    let bytes: Arc<[u8]> = std::fs::read(path)?.into();
    let document = match Document::open(bytes.clone(), &OpenOptions::default()) {
        Ok(doc) => doc,
        Err(error) => {
            println!("open-error\t{}", error.kind.name());
            return Ok(());
        }
    };
    let labels = usize::from(document.page_label_ranges().is_err());
    drop(document);
    let outlines = inspect(bytes.clone(), "outlines")?;
    let destinations = inspect(bytes.clone(), "destinations")?;
    let links = inspect(bytes, "links").unwrap_or_else(|error| {
        eprintln!(
            "pages\t{}\t{}",
            error
                .as_error()
                .map_or("unavailable", |error| error.kind.name()),
            error
        );
        Stats {
            errors: 1,
            ..Stats::default()
        }
    });
    println!(
        "ok\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{labels}\t{}\t{}\t{}",
        outlines.items,
        outlines.repairs,
        destinations.items,
        destinations.repairs,
        links.items,
        links.repairs,
        outlines.errors,
        destinations.errors,
        links.errors,
        usize::from(outlines.present),
        usize::from(destinations.present),
        usize::from(links.present)
    );
    Ok(())
}
