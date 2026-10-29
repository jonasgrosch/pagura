#![forbid(unsafe_code)]
// Independent G01 glue, using only hayro's documented public API.
use hayro::hayro_syntax::{
    Pdf,
    object::{Dict, Object},
};
use std::fmt::Write;

fn quoted(value: &[u8]) -> Result<String, &'static str> {
    // This slice probes ASCII Info strings and attachment filenames. Raw metadata bytes are not assumed
    // to be UTF-8: a future encoding probe must use a defined decoding contract.
    if !value.is_ascii() {
        return Err("PDF string encoding is outside this adapter's ASCII scope");
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

enum Failure {
    Adapter(&'static str),
    Limit(&'static str),
}

impl From<&'static str> for Failure {
    fn from(reason: &'static str) -> Self {
        Self::Adapter(reason)
    }
}

// This adapter only enumerates public objects and asks hayro to decode streams.
// Python hashes the observed bytes; no decoder, PDF string codec or hash is
// implemented here. Bounds describe the adapter, not engine feature support.
fn optional<'a>(dict: &Dict<'a>, key: &[u8]) -> Result<Option<Object<'a>>, Failure> {
    if !dict.contains_key(key) {
        return Ok(None);
    }
    let value = dict
        .get::<Object<'a>>(key)
        .ok_or("declared attachment field could not be resolved")?;
    Ok((!matches!(value, Object::Null(_))).then_some(value))
}

fn preferred<'a>(dict: &Dict<'a>) -> Result<Option<Object<'a>>, Failure> {
    match optional(dict, b"UF")? {
        Some(value) => Ok(Some(value)),
        None => optional(dict, b"F"),
    }
}

fn attachments(document: &Pdf, annotations: bool) -> Result<String, Failure> {
    const MAX_RECORDS: usize = 128;
    const MAX_WORK: usize = 1024;
    const MAX_BYTES: usize = 65536;
    let mut specs = Vec::new();
    if annotations {
        let page = document.pages().get(0).ok_or("missing first page")?;
        if let Some(value) = optional(page.raw(), b"Annots")? {
            let array = value.into_array().ok_or("Annots is not an array")?;
            for (index, object) in array.iter::<Object<'_>>().enumerate() {
                if index >= MAX_WORK {
                    return Err(Failure::Limit("annotation scan exceeds 1024 slots"));
                }
                let annotation = object.into_dict().ok_or("annotation is not a dictionary")?;
                let subtype = annotation
                    .get::<Object<'_>>(b"Subtype")
                    .and_then(Object::into_name)
                    .ok_or("missing annotation subtype")?;
                if subtype.as_str() == "FileAttachment" {
                    specs.push(
                        annotation
                            .get::<Dict<'_>>(b"FS")
                            .ok_or("missing annotation file specification")?,
                    );
                    if specs.len() > MAX_RECORDS {
                        return Err(Failure::Limit("attachment count exceeds 128"));
                    }
                }
            }
        }
    } else {
        let catalog = document
            .xref()
            .get::<Dict<'_>>(document.xref().root_id())
            .ok_or("missing catalog")?;
        let Some(names) = optional(&catalog, b"Names")? else {
            return Ok("[]".into());
        };
        let names = names.into_dict().ok_or("Names is not a dictionary")?;
        let Some(tree) = optional(&names, b"EmbeddedFiles")? else {
            return Ok("[]".into());
        };
        let mut pending = vec![tree];
        let mut work = 0;
        while let Some(node) = pending.pop() {
            work += 1;
            if work > MAX_WORK {
                return Err(Failure::Limit("name tree traversal exceeds 1024 items"));
            }
            let node = node
                .into_dict()
                .ok_or("name tree node is not a dictionary")?;
            if let Some(names) = optional(&node, b"Names")? {
                let names = names.into_array().ok_or("leaf Names is not an array")?;
                let mut values = names.iter::<Object<'_>>();
                while let Some(key) = values.next() {
                    key.into_string().ok_or("name tree key is not a string")?;
                    let spec = values
                        .next()
                        .and_then(Object::into_dict)
                        .ok_or("name tree file specification is missing")?;
                    specs.push(spec);
                    if specs.len() > MAX_RECORDS {
                        return Err(Failure::Limit("attachment count exceeds 128"));
                    }
                }
            }
            if let Some(kids) = optional(&node, b"Kids")? {
                let kids = kids.into_array().ok_or("Kids is not an array")?;
                let mut children = Vec::new();
                for child in kids.iter::<Object<'_>>() {
                    if work + pending.len() + children.len() >= MAX_WORK {
                        return Err(Failure::Limit("name tree traversal exceeds 1024 items"));
                    }
                    children.push(child);
                }
                pending.extend(children.into_iter().rev());
            }
        }
    }
    let mut records = Vec::new();
    let mut decoded_bytes = 0;
    for spec in specs {
        let name = preferred(&spec)?
            .and_then(Object::into_string)
            .ok_or("missing attachment filename")?;
        if name.as_bytes().len() > 1024 {
            return Err(Failure::Limit("filename exceeds 1024 bytes"));
        }
        let name = quoted(name.as_bytes())?;
        let ef = spec.get::<Dict<'_>>(b"EF").ok_or("missing EF dictionary")?;
        let stream = preferred(&ef)?
            .and_then(Object::into_stream)
            .ok_or("missing embedded payload stream")?;
        if optional(stream.dict(), b"F")?.is_some() {
            return Err("external streams are outside this adapter's scope".into());
        }
        let bytes = stream
            .decoded()
            .map_err(|_| "hayro could not decode the embedded stream")?;
        // The public API decodes completely before this aggregate output check.
        if bytes.len() > MAX_BYTES - decoded_bytes {
            return Err(Failure::Limit("decoded attachment output exceeds 64 KiB"));
        }
        decoded_bytes += bytes.len();
        let mut hex = String::with_capacity(bytes.len() * 2);
        for byte in bytes.iter() {
            write!(hex, "{byte:02x}").map_err(|_| "formatting failed")?;
        }
        records.push(format!(r#"{{"name":{name},"bytes_hex":"{hex}"}}"#));
    }
    Ok(format!("[{}]", records.join(",")))
}

fn value(document: &Pdf, operation: &str) -> Result<String, Failure> {
    match operation {
        "pages" => {
            let pages = document.pages();
            let mut sizes = Vec::new();
            for index in 0..pages.len() {
                let page = pages.get(index).ok_or("missing page")?;
                let (width, height) = page.render_dimensions();
                if !width.is_finite() || !height.is_finite() {
                    return Err("nonfinite page dimensions".into());
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
        "attachments" => attachments(document, false),
        "annotation-attachments" => attachments(document, true),
        _ => Err("operation is not implemented by this adapter".into()),
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
            Err(failure) => {
                let (status, reason) = match failure {
                    Failure::Adapter(reason) => ("adapter-error", reason),
                    Failure::Limit(reason) => ("limit", reason),
                };
                println!(
                    r#"{{"schema":1,"status":"{status}","reason":{}}}"#,
                    quoted(reason.as_bytes())?
                );
            }
        },
    }
    Ok(())
}
