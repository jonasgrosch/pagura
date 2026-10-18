//! Preserve existing artwork and resource precedence (12.7.4.3).
use super::Program;
use pagura_core::{
    Dictionary, Error, Limits, Result, Value,
    syntax::{Origin, Parser},
};
use pagura_document::Document;

pub(super) fn resources(
    document: &Document,
    existing: Option<&Dictionary>,
    defaults: Option<&Dictionary>,
) -> Result<Dictionary> {
    let bytes = existing
        .map_or(0, Dictionary::retained_bytes)
        .saturating_add(defaults.map_or(0, Dictionary::retained_bytes));
    if bytes > document.limits().document_bytes {
        return Err(Error::limit("appearance resources storage budget exceeded"));
    }
    let mut result = existing.cloned().unwrap_or_default();
    if let Some(defaults) = defaults {
        for (category, value) in defaults {
            let value_resolved = document.resolve(value)?;
            if value_resolved.is_null() {
                continue;
            }
            if let Some(old) = result.get(category.as_bytes()).filter(|v| !v.is_null()) {
                let old = document.resolve(old)?;
                if old.is_null() {
                    result.insert(category.clone(), value.clone());
                    continue;
                }
                let new = value_resolved;
                let (Some(old), Some(new)) = (old.as_dictionary(), new.as_dictionary()) else {
                    continue;
                };
                if old.retained_bytes().saturating_add(new.retained_bytes())
                    > document.limits().document_bytes
                {
                    return Err(Error::limit(
                        "merged appearance resources storage budget exceeded",
                    ));
                }
                let mut merged = old.clone();
                for (name, value) in new {
                    if super::optional(document, &merged, name.as_bytes())?.is_none() {
                        merged.insert(name.clone(), value.clone());
                    }
                }
                result.insert(category.clone(), Value::Dictionary(merged));
            } else {
                result.insert(category.clone(), value.clone());
            }
        }
    }
    Ok(result)
}
pub(super) fn replace(bytes: &[u8], text: &[u8], limits: Limits) -> Result<Vec<u8>> {
    let region = text_region(bytes, limits)?;
    let mut out = Program::new(limits);
    if let Some(region) = region {
        out.bytes(&bytes[..region.start])?;
        out.bytes(text)?;
        out.bytes(&bytes[region.end..])?;
    } else {
        out.bytes(bytes)?;
        out.text("\n")?;
        out.bytes(text)?;
    }
    Ok(out.finish())
}

fn text_region(bytes: &[u8], limits: Limits) -> Result<Option<std::ops::Range<usize>>> {
    if bytes.len() > limits.decoded_bytes {
        return Err(Error::limit("existing appearance byte budget exceeded"));
    }
    let mut parser = Parser::new(bytes, Origin::Buffer, limits);
    let mut operands = Vec::new();
    let (mut start, mut depth, mut active, mut region) = (0, 0usize, None, None);
    let mut work = 0usize;
    let mut scope = Scope::default();
    let mut start_scope = Scope::default();
    while let Some(next) = parser.peek() {
        if work >= limits.operations {
            return Err(
                Error::limit("appearance update token budget exceeded").at(parser.position())
            );
        }
        work += 1;
        if operands.is_empty() {
            start = parser.offset();
        }
        if matches!(
            next,
            b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.' | b'0'..=b'9'
        ) || parser.data().get(parser.offset()..).is_some_and(|b| {
            b.starts_with(b"true") || b.starts_with(b"false") || b.starts_with(b"null")
        }) {
            if operands.len() >= limits.value_nodes {
                return Err(Error::limit("appearance update operand budget exceeded"));
            }
            operands.push(parser.parse()?);
            continue;
        }
        let operator = parser.word()?;
        scope.step(operator, active.map(|_| start_scope.graphics), limits)?;
        match operator {
            b"BMC" | b"BDC" => {
                let tx = operands
                    .first()
                    .and_then(Value::as_name)
                    .is_some_and(|n| n.as_bytes() == b"Tx");
                if tx && active.is_none() {
                    if region.is_some() {
                        return Err(Error::unsupported(
                            crate::form_appearance::features::APPEARANCE_UPDATE,
                            "multiple variable text sections are unavailable",
                        ));
                    }
                    if scope.text {
                        return Err(Error::unsupported(
                            crate::form_appearance::features::APPEARANCE_UPDATE,
                            "variable text section inside a text object is unavailable",
                        ));
                    }
                    start_scope = scope;
                    active = Some((start, depth));
                }
                depth = depth
                    .checked_add(1)
                    .filter(|d| *d <= limits.depth)
                    .ok_or(Error::limit("appearance marked-content depth exceeded"))?;
            }
            b"EMC" => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(Error::syntax("appearance EMC without BMC"))?;
                if let Some((begin, level)) = active
                    && level == depth
                {
                    if scope != start_scope {
                        return Err(Error::unsupported(
                            crate::form_appearance::features::APPEARANCE_UPDATE,
                            "variable text section crosses a graphics or text scope",
                        ));
                    }
                    region = Some(begin..parser.offset());
                    active = None;
                }
            }
            b"BI" => {
                return Err(Error::unsupported(
                    crate::form_appearance::features::APPEARANCE_UPDATE,
                    "updating appearances with inline images is unavailable",
                ));
            }
            _ => (),
        }
        operands.clear();
    }
    if depth != 0 || scope != Scope::default() || !operands.is_empty() {
        return Err(Error::syntax("unfinished appearance content"));
    }
    Ok(region)
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct Scope {
    graphics: usize,
    text: bool,
}
impl Scope {
    fn step(&mut self, op: &[u8], minimum: Option<usize>, limits: Limits) -> Result<()> {
        match op {
            b"BT" => {
                if self.text {
                    return Err(Error::syntax("nested appearance text object"));
                }
                self.text = true;
            }
            b"ET" => {
                if !self.text {
                    return Err(Error::syntax("appearance ET without BT"));
                }
                self.text = false;
            }
            b"q" => {
                self.graphics = self
                    .graphics
                    .checked_add(1)
                    .filter(|n| *n <= limits.depth)
                    .ok_or(Error::limit("appearance graphics stack budget exceeded"))?;
            }
            b"Q" => {
                if minimum.is_some_and(|n| self.graphics <= n) {
                    return Err(Error::unsupported(
                        crate::form_appearance::features::APPEARANCE_UPDATE,
                        "variable text section crosses a graphics scope",
                    ));
                }
                self.graphics = self
                    .graphics
                    .checked_sub(1)
                    .ok_or(Error::syntax("appearance Q without q"))?;
            }
            _ => (),
        }
        Ok(())
    }
}
