#![forbid(unsafe_code)]
//! Hosted pixel, source-identity, link and structural policy qualification.
//! Placeholder signature fixtures assert no cryptographic integrity.
use pagura::signatures::PermittedModifications;
use pagura::{Document, OpenOptions, Recovery, RenderOptions, Rgba8};
use std::{error::Error, fs, sync::Arc};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("input required")?;
    let output = args.next().ok_or("output required")?;
    let recovery = match args.next().as_deref() {
        Some("repair") => Recovery::Repair,
        Some("strict") => Recovery::Strict,
        _ => return Err("expected strict or repair".into()),
    };
    let dpi: f64 = args.next().ok_or("dpi required")?.parse()?;
    let operation = args.next().ok_or("render, links or policy required")?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let source = fs::read(input)?;
    let document = Arc::new(Document::open(
        source.clone(),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )?);
    let bytes = match operation.as_str() {
        "render" => {
            let context = pagura::render_context(Arc::clone(&document));
            let options = RenderOptions {
                background: Rgba8::WHITE,
                ..RenderOptions::from_dpi(dpi)
            };
            let pixels = context.render_page(document.page(0)?, &options)?;
            let mut bytes =
                format!("P6\n{} {}\n255\n", pixels.width(), pixels.height()).into_bytes();
            bytes.extend(pixels.to_rgb_over(Rgba8::WHITE));
            bytes
        }
        "links" => {
            let page = document.page(0)?;
            let links = page.links(&document)?;
            let values = page.dictionary().get(b"Annots").ok_or("Annots required")?;
            let values = document
                .resolve(values)?
                .as_array()
                .ok_or("array required")?;
            for link in &links {
                let mut original = false;
                for id in values.iter().filter_map(pagura::Value::as_reference) {
                    if document
                        .object(id)?
                        .dictionary()
                        .is_some_and(|d| std::ptr::eq(link.dictionary, d))
                    {
                        original = true;
                    }
                }
                if !original {
                    return Err("link dictionary lost original identity".into());
                }
            }
            println!("link-identity-preserved");
            format!("{}\n", links.len()).into_bytes()
        }
        "policy" => {
            let signatures = document.signatures()?;
            let signature = signatures
                .fields
                .iter()
                .find(|field| field.qualified_name == "Approval")
                .and_then(|field| field.signature.as_ref())
                .ok_or("authored Approval signature required")?;
            match &signature.results.permitted_modifications {
                PermittedModifications::Analysed(analysis) => {
                    format!("{:?}\n", analysis.outcome).into_bytes()
                }
                PermittedModifications::Inconclusive => b"Inconclusive\n".to_vec(),
                _ => return Err("structural policy result unavailable".into()),
            }
        }
        _ => return Err("unknown qualification operation".into()),
    };
    if document.original_bytes() != source.as_slice() {
        return Err("original source bytes changed".into());
    }
    println!("original-source-preserved");
    // Fresh hosted scratch paths are supplied by the runner, never the PDF.
    std::io::Write::write_all(
        &mut fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?,
        &bytes,
    )?;
    for diagnostic in document.diagnostics() {
        println!("{}", diagnostic.tag());
    }
    Ok(())
}
