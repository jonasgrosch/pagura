//! Preserve existing artwork and resource precedence (12.7.4.3).

use super::{UpdatePolicy, compatibility};
use crate::features;
use crate::program::Program;
use crate::values::optional;
use pagura_core::{
    Dictionary, Error, Limits, Result, Value,
    syntax::{Origin, Parser},
};
use pagura_document::Document;
use std::ops::Range;

/// The existing appearance's resources with the form's default resources
/// (DR) added; existing entries win collisions within each category.
pub(super) fn resources(
    document: &Document,
    existing: Option<&Dictionary>,
    defaults: Option<&Dictionary>,
) -> Result<Dictionary> {
    let limits = document.limits();
    let bytes = existing
        .map_or(0, Dictionary::retained_bytes)
        .saturating_add(defaults.map_or(0, Dictionary::retained_bytes));
    if bytes > limits.document_bytes {
        return Err(Error::limit("appearance resources storage budget exceeded"));
    }
    let mut result = existing.cloned().unwrap_or_default();
    let Some(defaults) = defaults else {
        return Ok(result);
    };
    for (category, value) in defaults {
        let new = document.resolve(value)?;
        if new.is_null() {
            continue;
        }
        let Some(old) = result.get(category.as_bytes()).filter(|v| !v.is_null()) else {
            result.insert(category.clone(), value.clone());
            continue;
        };
        let old = document.resolve(old)?;
        if old.is_null() {
            result.insert(category.clone(), value.clone());
            continue;
        }
        let (Some(old), Some(new)) = (old.as_dictionary(), new.as_dictionary()) else {
            continue;
        };
        if old.retained_bytes().saturating_add(new.retained_bytes()) > limits.document_bytes {
            return Err(Error::limit(
                "merged appearance resources storage budget exceeded",
            ));
        }
        let mut merged = old.clone();
        for (name, value) in new {
            if optional(document, &merged, name.as_bytes())?.is_none() {
                merged.insert(name.clone(), value.clone());
            }
        }
        result.insert(category.clone(), Value::Dictionary(merged));
    }
    Ok(result)
}

/// The existing content with its Tx section replaced by `text`; without a
/// section, `text` follows the content, after the content's unmarked text
/// is removed under [`UpdatePolicy::PreserveExisting`].
pub(super) fn replace(
    bytes: &[u8],
    text: &[u8],
    limits: &Limits,
    policy: UpdatePolicy,
) -> Result<Vec<u8>> {
    let region = text_region(bytes, limits)?;
    let mut out = Program::new(limits);
    if let Some(region) = region {
        let (before, after) = bytes
            .get(..region.start)
            .zip(bytes.get(region.end..))
            .ok_or(Error::syntax("invalid variable text section"))?;
        out.bytes(before)?;
        out.bytes(text)?;
        out.bytes(after)?;
    } else if policy == UpdatePolicy::PreserveExisting {
        out.text("q\n")?;
        out.bytes(&compatibility::without_text(bytes, limits)?)?;
        out.text("\nQ\n")?;
        out.bytes(text)?;
    } else {
        out.bytes(bytes)?;
        out.text("\n")?;
        out.bytes(text)?;
    }
    Ok(out.finish())
}

/// Whether the parser is at an operand rather than an operator.
pub(super) fn at_operand(parser: &Parser<'_>, next: u8) -> bool {
    matches!(
        next,
        b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.' | b'0'..=b'9'
    ) || parser.data().get(parser.offset()..).is_some_and(|b| {
        b.starts_with(b"true") || b.starts_with(b"false") || b.starts_with(b"null")
    })
}

/// The byte range of the one top-level `/Tx BMC … EMC` section, which must
/// not cross a text object or graphics state scope.
fn text_region(bytes: &[u8], limits: &Limits) -> Result<Option<Range<usize>>> {
    if bytes.len() > limits.decoded_bytes {
        return Err(Error::limit("existing appearance byte budget exceeded"));
    }
    let mut parser = Parser::new(bytes, Origin::Buffer, *limits);
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
        if at_operand(&parser, next) {
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
                            features::APPEARANCE_UPDATE,
                            "multiple variable text sections are unavailable",
                        ));
                    }
                    if scope.text {
                        return Err(Error::unsupported(
                            features::APPEARANCE_UPDATE,
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
                            features::APPEARANCE_UPDATE,
                            "variable text section crosses a graphics or text scope",
                        ));
                    }
                    region = Some(begin..parser.offset());
                    active = None;
                }
            }
            b"BI" => {
                return Err(Error::unsupported(
                    features::APPEARANCE_UPDATE,
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

/// The graphics-state depth and text-object state of a content position.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
struct Scope {
    graphics: usize,
    text: bool,
}

impl Scope {
    /// Follows `op`; `minimum` is the graphics depth an open Tx section
    /// started at, which a `Q` may not leave.
    fn step(&mut self, op: &[u8], minimum: Option<usize>, limits: &Limits) -> Result<()> {
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
                        features::APPEARANCE_UPDATE,
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
