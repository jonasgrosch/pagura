#![forbid(unsafe_code)]
// Independent G01 glue, using only hayro's documented public API.
use hayro::hayro_syntax::Pdf;
use std::fmt::Write;

fn quoted(value: &[u8]) -> Result<String, &'static str> {
    // This slice probes ASCII Info strings. Raw metadata bytes are not assumed
    // to be UTF-8: a future encoding probe must use a defined decoding contract.
    if !value.is_ascii() {
        return Err("metadata encoding is outside this adapter's ASCII scope");
    }
    let mut output = String::from("\"");
    for &byte in value {
        match byte {
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            0..=31 => write!(output, "\\u{byte:04x}").map_err(|_| "formatting failed")?,
            _ => output.push(char::from(byte)),
        }
    }
    output.push('"');
    Ok(output)
}

fn value(document: &Pdf, operation: &str) -> Result<String, &'static str> {
    match operation {
        "pages" => {
            let pages = document.pages();
            let mut sizes = Vec::new();
            for index in 0..pages.len() {
                let page = pages.get(index).ok_or("missing page")?;
                let (width, height) = page.render_dimensions();
                if !width.is_finite() || !height.is_finite() {
                    return Err("nonfinite page dimensions");
                }
                sizes.push(format!("[{width},{height}]"));
            }
            Ok(format!(
                "{{\"count\":{},\"sizes\":[{}]}}",
                pages.len(),
                sizes.join(",")
            ))
        }
        "metadata" => {
            let metadata = document.metadata();
            let title = quoted(metadata.title.as_deref().unwrap_or_default())?;
            let author = quoted(metadata.author.as_deref().unwrap_or_default())?;
            Ok(format!("{{\"Title\":{title},\"Author\":{author}}}"))
        }
        _ => Err("operation is not implemented by this adapter"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.as_slice() == ["--version"] {
        println!(r#"{{"engine":"hayro 0.8.0","wrapper":"hayro-syntax public API 0.8.0"}}"#);
        return Ok(());
    }
    if args.len() != 2 {
        return Err("expected OPERATION SYNTHETIC_PDF".into());
    }
    match Pdf::new(std::fs::read(&args[1])?) {
        Err(_) => println!(r#"{{"schema":1,"status":"rejected","reason":"LoadPdfError"}}"#),
        Ok(document) => match value(&document, &args[0]) {
            Ok(value) => println!(r#"{{"schema":1,"status":"ok","value":{value}}}"#),
            Err(reason) => println!(
                r#"{{"schema":1,"status":"adapter-error","reason":{}}}"#,
                quoted(reason.as_bytes())?
            ),
        },
    }
    Ok(())
}
