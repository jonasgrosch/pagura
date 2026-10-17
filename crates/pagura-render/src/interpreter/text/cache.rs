//! Per-compilation shared font, outline and image storage admission.
use super::{Arc, Compiler, Dictionary, Error, PdfFont, Result, Value};
use pagura_raster::Path;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(in crate::interpreter) struct Cache {
    pub(super) fonts: BTreeMap<pagura_core::ObjectId, Arc<PdfFont>>,
    direct_fonts: Vec<(Dictionary, Arc<PdfFont>)>,
    resources: Vec<Dictionary>,
    pub(in crate::interpreter) page_resources: Option<Arc<Dictionary>>,
    pub(super) outlines: BTreeMap<(usize, u32), Arc<Path>>,
    pub(in crate::interpreter) images:
        BTreeMap<(pagura_core::ObjectId, usize), Arc<super::super::images::Image>>,
    pub(super) bytes: usize,
    pub(super) entries: usize,
}
impl Compiler<'_> {
    pub(in crate::interpreter) fn paint_charge(&self, bytes: usize, entries: usize) -> Result<()> {
        let limits = self.document.limits();
        let mut cache = self.state.text.cache.borrow_mut();
        let bytes = cache
            .bytes
            .checked_add(bytes)
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or(Error::limit("text and image retained byte budget exceeded"))?;
        let entries = cache
            .entries
            .checked_add(entries)
            .filter(|&n| {
                n.checked_add(self.marks)
                    .is_some_and(|total| total <= limits.display_commands)
            })
            .ok_or(Error::limit("text and image cache entry budget exceeded"))?;
        cache.bytes = bytes;
        cache.entries = entries;
        Ok(())
    }
    pub(in crate::interpreter) fn resource_identity(
        &self,
        resources: &Dictionary,
    ) -> Result<usize> {
        if let Some(key) = self
            .state
            .text
            .cache
            .borrow()
            .resources
            .iter()
            .position(|r| r == resources)
        {
            return Ok(key);
        }
        self.paint_charge(resources.retained_bytes(), 1)?;
        let mut cache = self.state.text.cache.borrow_mut();
        if cache.resources.len() >= 256 {
            return Err(Error::limit("image resource cache budget exceeded"));
        }
        let key = cache.resources.len();
        cache.resources.push(resources.clone());
        Ok(key)
    }
    pub(super) fn select_font(&mut self, value: &Value, size: f64) -> Result<()> {
        let dictionary = self.document.resolve_dictionary(value)?;
        let key = value.as_reference();
        let cached = {
            let cache = self.state.text.cache.borrow();
            if let Some(key) = key {
                cache.fonts.get(&key).cloned()
            } else {
                cache
                    .direct_fonts
                    .iter()
                    .find(|(d, _)| d == dictionary)
                    .map(|(_, font)| Arc::clone(font))
            }
        };
        let font = if let Some(font) = cached {
            font
        } else {
            self.paint_charge(
                dictionary
                    .retained_bytes()
                    .saturating_add(font_input_bytes(self.document, dictionary, 0)?)
                    .saturating_add(size_of::<PdfFont>()),
                1,
            )?;
            let font = Arc::new(PdfFont::load(self.document, dictionary).map_err(
                |mut error| {
                    if matches!(
                        error.position,
                        pagura_core::Position::Unknown | pagura_core::Position::Buffer(_)
                    ) {
                        error.position = self.position;
                    }
                    key.map_or(error.clone(), |id| error.in_object(id))
                },
            )?);
            let mut cache = self.state.text.cache.borrow_mut();
            if cache.fonts.len() + cache.direct_fonts.len() >= 256 {
                return Err(Error::limit("font cache budget exceeded"));
            }
            if let Some(key) = key {
                cache.fonts.insert(key, Arc::clone(&font));
            } else {
                cache
                    .direct_fonts
                    .push((dictionary.clone(), Arc::clone(&font)));
            }
            font
        };
        self.state.text.font = Some(font);
        self.state.text.size = size;
        Ok(())
    }
    // Text entries are separated before delegating the remaining ExtGState to
    // the existing implementation, preserving its precedence and validation.
    pub(in crate::interpreter) fn paint_gstate(
        &mut self,
        values: &[Value],
        resources: &Dictionary,
    ) -> Result<()> {
        let name = values
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("ExtGState name expected"))?;
        let value = super::resource(self.document, resources, b"ExtGState", name.as_bytes())?;
        let dict = self.document.resolve_dictionary(value)?;
        let uncolored = self.uncolored_glyph();
        if let Some(font) = dict.get(b"Font") {
            let array = self
                .document
                .resolve(font)?
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or(Error::syntax("ExtGState Font requires font and size"))?;
            let size = self
                .document
                .resolve(&array[1])?
                .as_number()
                .ok_or(Error::syntax("invalid ExtGState font size"))?;
            self.select_font(&array[0], size)?;
        }
        if let Some(knockout) = dict.get(b"TK") {
            let knockout = self
                .document
                .resolve(knockout)?
                .as_bool()
                .ok_or(Error::syntax("TK must be boolean"))?;
            if self.state.text.object.is_none() && !uncolored {
                self.state.text.knockout = knockout;
            }
        }
        if !dict.contains_key(b"Font") && !uncolored {
            return self.extgstate(values, resources);
        }
        let filtered: Dictionary = dict
            .iter()
            .filter(|(k, _)| *k != b"Font" && !(uncolored && color_parameter(k.as_bytes())))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut states = Dictionary::new();
        states.insert(name.clone(), Value::Dictionary(filtered));
        let mut local = resources.clone();
        local.insert(
            pagura_core::Name::new(b"ExtGState"),
            Value::Dictionary(states),
        );
        self.extgstate(values, &local)
    }
}

fn color_parameter(key: &[u8]) -> bool {
    matches!(
        key,
        b"RI"
            | b"BG"
            | b"BG2"
            | b"UCR"
            | b"UCR2"
            | b"TR"
            | b"TR2"
            | b"HT"
            | b"UseBlackPtComp"
            | b"CA"
            | b"ca"
            | b"BM"
            | b"SMask"
            | b"AIS"
            | b"OP"
            | b"op"
            | b"OPM"
            | b"TK"
    )
}

fn font_input_bytes(
    document: &pagura_document::Document,
    dictionary: &Dictionary,
    depth: usize,
) -> Result<usize> {
    if depth >= document.limits().depth {
        return Err(Error::limit("font resource nesting exceeded"));
    }
    let mut bytes = 0_usize;
    if let Some(descriptor) = dictionary.get(b"FontDescriptor") {
        for (key, value) in document.resolve_dictionary(descriptor)?.iter() {
            if matches!(key.as_bytes(), b"FontFile" | b"FontFile2" | b"FontFile3") {
                let id = value
                    .as_reference()
                    .ok_or(Error::syntax("font program must be indirect"))?;
                bytes = bytes
                    .checked_add(document.stream_data(document.object(id)?)?.len())
                    .ok_or(Error::limit("font input byte overflow"))?;
            }
        }
    }
    if dictionary.is_name(b"Subtype", b"Type0")
        && let Some(descendants) = dictionary.get(b"DescendantFonts")
    {
        let descendants = document
            .resolve(descendants)?
            .as_array()
            .filter(|values| values.len() == 1)
            .ok_or(Error::syntax("Type0 requires one descendant font"))?;
        let descendant = document.resolve_dictionary(&descendants[0])?;
        if !descendant.is_name(b"Subtype", b"CIDFontType0")
            && !descendant.is_name(b"Subtype", b"CIDFontType2")
        {
            return Err(Error::syntax("invalid CIDFont subtype"));
        }
        bytes = bytes
            .checked_add(font_input_bytes(document, descendant, depth + 1)?)
            .ok_or(Error::limit("font input byte overflow"))?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_font_fanout_is_rejected_before_loading_descendants() {
        let document = pagura_document::Document::open(
            pagura_testkit::fixture("paint", "text-mode-0.pdf"),
            &pagura_document::OpenOptions::default(),
        )
        .unwrap();
        let dictionary: Dictionary = [
            (pagura_core::Name::new(b"Subtype"), Value::name(b"Type0")),
            (
                pagura_core::Name::new(b"DescendantFonts"),
                Value::Array(vec![Value::Reference(pagura_core::ObjectId::new(5, 0)); 2]),
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            font_input_bytes(&document, &dictionary, 0)
                .unwrap_err()
                .kind,
            pagura_core::ErrorKind::Syntax
        );
    }
}
