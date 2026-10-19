//! Framing of ignored inline colour images in stencil patterns (8.6.8; 8.9.7).
//! Only the header and encoded length are examined; ignored samples are not decoded.
use super::{Compiler, Dictionary, Error, Result, Value, resource};
use pagura_core::Name;
use pagura_core::syntax::{Origin, Parser};
impl Compiler<'_> {
    pub(super) fn pattern_inline(
        &mut self,
        parser: &mut Parser<'_>,
        resources: &Dictionary,
    ) -> Result<()> {
        if !self.stencil_pattern {
            return self.inline_image(parser, resources);
        }
        if !self.operands.is_empty() {
            return Err(Error::syntax("operands before BI"));
        }
        let start = parser.offset();
        let mut probe =
            Parser::with_offset(parser.data(), start, Origin::Buffer, self.document.limits())?;
        let mut d = Dictionary::new();
        while probe.peek() != Some(b'I') {
            self.operations = self
                .operations
                .checked_add(1)
                .filter(|n| *n <= self.document.limits().operations)
                .ok_or(Error::limit("inline image header budget exceeded"))?;
            let name = probe
                .parse()?
                .as_name()
                .cloned()
                .ok_or(Error::syntax("inline image key must be name"))?;
            let v = probe.parse()?;
            super::no_references(&v)?;
            if d.contains_key(name.as_bytes()) {
                return Err(Error::syntax("duplicate inline image key"));
            }
            d.insert(name, v);
        }
        if probe.word() != Ok(b"ID") {
            return Err(Error::syntax("inline image requires ID"));
        }
        if d.get(b"IM")
            .or_else(|| d.get(b"ImageMask"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            return self.inline_image(parser, resources);
        }
        let offset = probe.offset();
        let byte = *probe
            .data()
            .get(offset)
            .filter(|b| matches!(**b, 0 | 9 | 10 | 12 | 13 | 32))
            .ok_or(Error::syntax("ID requires whitespace"))?;
        let offset = offset
            + if byte == 13 && probe.data().get(offset + 1) == Some(&10) {
                2
            } else {
                1
            };
        if let Some(name) = d
            .get(b"CS")
            .or_else(|| d.get(b"ColorSpace"))
            .and_then(Value::as_name)
            .filter(|n| {
                !matches!(
                    n.as_bytes(),
                    b"G" | b"RGB" | b"CMYK" | b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"
                )
            })
        {
            let space = resource(self.document, resources, b"ColorSpace", name.as_bytes())?;
            d.insert(
                Name::new(b"ColorSpace"),
                self.document.resolve(space)?.clone(),
            );
            d.remove(b"CS");
        }
        let length = pagura_image::inline_data_length(
            &d,
            probe
                .data()
                .get(offset..)
                .ok_or(Error::syntax("missing inline image data"))?,
            &self.document.limits(),
        )?;
        probe.seek(
            offset
                .checked_add(length)
                .ok_or(Error::limit("inline image offset overflow"))?,
        )?;
        if probe
            .data()
            .get(probe.offset())
            .is_none_or(|b| !matches!(*b, 0 | 9 | 10 | 12 | 13 | 32))
            || probe.word() != Ok(b"EI")
        {
            return Err(Error::syntax("inline image requires EI"));
        }
        parser.seek(probe.offset())
    }
}
