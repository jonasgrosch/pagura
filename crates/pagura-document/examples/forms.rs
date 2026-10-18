#![forbid(unsafe_code)]
//! Inspects read-only form data, without executing actions or parsing XFA.
//! Usage: cargo run --release -p pagura-document --example forms -- FILE
use pagura_document::{
    Document, OpenOptions,
    forms::{ButtonKind, FieldKind},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("expected an input PDF path")?;
    let document = match Document::open(std::fs::read(path)?, &OpenOptions::default()) {
        Ok(document) => document,
        Err(error) => {
            println!("open-error\t{}", error.kind.name());
            return Ok(());
        }
    };
    let before = document.diagnostics().len();
    match document.acro_form() {
        Ok(None) => println!("absent"),
        Ok(Some(form)) => {
            let mut counts = [0usize; 8];
            for field in &form.fields {
                let index = match field.kind {
                    FieldKind::Group => 0,
                    FieldKind::Button {
                        kind: ButtonKind::Push,
                        ..
                    } => 1,
                    FieldKind::Button {
                        kind: ButtonKind::CheckBox,
                        ..
                    } => 2,
                    FieldKind::Button {
                        kind: ButtonKind::Radio,
                        ..
                    } => 3,
                    FieldKind::Text { .. } => 4,
                    FieldKind::Choice { .. } => 5,
                    FieldKind::Signature { .. } => 6,
                    FieldKind::Other(_) => 7,
                };
                counts[index] += 1;
            }
            println!(
                "ok\t{}\t{}\t{}\t{}",
                usize::from(form.has_xfa),
                document.diagnostics().len() - before,
                form.widgets.len(),
                counts.map(|n| n.to_string()).join(",")
            );
        }
        Err(error) => {
            println!("form-error\t{}\t{}", error.kind.name(), error);
        }
    }
    for d in document.diagnostics().iter().skip(before) {
        eprintln!("{}", d.repair.as_str());
    }
    Ok(())
}
