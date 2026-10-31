#![forbid(unsafe_code)]
//! Hosted native/WASI independent oracle controls for compatibility gaps.
use pagura::{
    Document, ErrorKind, Limits, ObjectId, OpenOptions, Recovery, RenderOptions, Rgba8, Value,
};
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
    let mut stream_bytes = Limits::default().stream_bytes;
    let mut explicit_capacity = false;
    let mut queries = Vec::new();
    for argument in args {
        if let Some(query) = argument.strip_prefix("query=") {
            queries.push(Query::parse(query)?);
        } else if !explicit_capacity {
            stream_bytes = argument.parse()?;
            explicit_capacity = true;
        } else {
            return Err("unexpected argument".into());
        }
    }
    let original = fs::read(input)?;
    let document = Arc::new(Document::open(
        original.clone(),
        &OpenOptions {
            recovery,
            limits: Limits {
                stream_bytes,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )?);
    for query in &queries {
        query.verify(&document)?;
    }
    if !queries.is_empty() {
        println!("document-query-contracts=pass");
    }
    let context = pagura::render_context(Arc::clone(&document));
    let options = RenderOptions {
        background: Rgba8::WHITE,
        ..RenderOptions::from_dpi(dpi)
    };
    let pixels = context.render_page(document.page(0)?, &options)?;
    if document.original_bytes() != original {
        return Err("source bytes changed".into());
    }
    let mut bytes = format!("P6\n{} {}\n255\n", pixels.width(), pixels.height()).into_bytes();
    bytes.extend(pixels.to_rgb_over(Rgba8::WHITE));
    // These are fresh hosted scratch paths, never document-selected paths.
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

struct Query {
    id: ObjectId,
    expected: ExpectedValue,
}

enum ExpectedValue {
    Missing,
    Integer(i64),
    Array(i64),
    Name(Vec<u8>),
    String(Vec<u8>),
}

impl Query {
    fn parse(source: &str) -> Result<Self, Box<dyn Error>> {
        let mut fields = source.splitn(4, ':');
        let number = fields.next().ok_or("query number required")?.parse()?;
        let generation = fields.next().ok_or("query generation required")?.parse()?;
        let kind = fields.next().ok_or("query kind required")?;
        let value = fields.next();
        let expected = match kind {
            "missing" if value.is_none() => ExpectedValue::Missing,
            "integer" => ExpectedValue::Integer(value.ok_or("integer value required")?.parse()?),
            "array" => ExpectedValue::Array(value.ok_or("array integer required")?.parse()?),
            "name" => ExpectedValue::Name(value.ok_or("name value required")?.as_bytes().to_vec()),
            "string" => {
                ExpectedValue::String(value.ok_or("string value required")?.as_bytes().to_vec())
            }
            _ => return Err("invalid query contract".into()),
        };
        Ok(Self {
            id: ObjectId::new(number, generation),
            expected,
        })
    }

    fn verify(&self, document: &Document) -> Result<(), Box<dyn Error>> {
        if matches!(self.expected, ExpectedValue::Missing) {
            return match document.object(self.id) {
                Err(error)
                    if error.kind == ErrorKind::MissingObject
                        && !document.contains_object(self.id) =>
                {
                    Ok(())
                }
                Err(error) => Err(error.into()),
                Ok(_) => Err("unexpected document object".into()),
            };
        }
        let object = document.object(self.id)?;
        let matches = match (&self.expected, object.value()) {
            (ExpectedValue::Integer(expected), Value::Integer(value)) => value == expected,
            (ExpectedValue::Array(expected), Value::Array(values)) => {
                matches!(values.as_slice(), [Value::Integer(value)] if value == expected)
            }
            (ExpectedValue::Name(expected), Value::Name(value)) => {
                value.as_bytes() == expected.as_slice()
            }
            (ExpectedValue::String(expected), Value::String(value)) => value == expected,
            _ => false,
        };
        if !matches
            || !document.contains_object(self.id)
            || !std::ptr::eq(object, document.object(self.id)?)
        {
            return Err("document query contract mismatch".into());
        }
        Ok(())
    }
}
