//! A temporary descriptor view for the existing PDF font loader.
use super::{Dictionary, Document, Error, Name, Result, Value};

pub(super) struct Descriptor<'a> {
    font: &'a Dictionary,
    metrics: &'a Dictionary,
    values: &'a Dictionary,
    composite: bool,
    pub object: Option<pagura_core::ObjectId>,
}
impl<'a> Descriptor<'a> {
    pub fn find(document: &'a Document, font: &'a Dictionary) -> Result<Option<Self>> {
        if font.is_name(b"Subtype", b"Type3") {
            return Ok(None);
        }
        let composite = font.is_name(b"Subtype", b"Type0");
        let metrics = if composite {
            let Some(value) = font.get(b"DescendantFonts") else {
                return Ok(None);
            };
            let Some([value]) = document.resolve(value)?.as_array() else {
                return Ok(None);
            };
            document.resolve_dictionary(value)?
        } else {
            font
        };
        let Some(value) = metrics.get(b"FontDescriptor") else {
            return Ok(None);
        };
        if document.resolve(value)?.is_null() {
            return Ok(None);
        }
        Ok(Some(Self {
            font,
            metrics,
            values: document.resolve_dictionary(value)?,
            composite,
            object: value.as_reference(),
        }))
    }
    pub fn flags(&self, document: &Document) -> Result<Option<i64>> {
        self.values
            .get(b"Flags")
            .map(|v| document.resolve(v))
            .transpose()
            .map(|v| v.and_then(Value::as_integer))
    }
    pub fn corrected(&self, document: &Document, symbolic: bool) -> Result<Dictionary> {
        let flags = self
            .flags(document)?
            .ok_or(Error::syntax("invalid font descriptor Flags"))?;
        let bytes = self
            .font
            .retained_bytes()
            .saturating_add(self.metrics.retained_bytes())
            .saturating_add(self.values.retained_bytes());
        if bytes > document.limits().decoded_bytes {
            return Err(Error::limit(
                "font descriptor recovery byte budget exceeded",
            ));
        }
        let mut descriptor = self.values.clone();
        descriptor.insert(
            Name::new(b"Flags"),
            Value::Integer((flags & !0x24) | if symbolic { 4 } else { 32 }),
        );
        let mut metrics = self.metrics.clone();
        metrics.insert(Name::new(b"FontDescriptor"), Value::Dictionary(descriptor));
        if !self.composite {
            return Ok(metrics);
        }
        let mut font = self.font.clone();
        font.insert(
            Name::new(b"DescendantFonts"),
            Value::Array(vec![Value::Dictionary(metrics)]),
        );
        Ok(font)
    }
}
