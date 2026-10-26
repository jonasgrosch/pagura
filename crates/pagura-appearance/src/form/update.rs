//! Preserve existing artwork and resource precedence (12.7.4.3).

use super::UpdatePolicy;
use crate::features;
use crate::program::Program;
use crate::values::optional;
use pagura_content::{Content, Op, Reader};
use pagura_core::{Dictionary, Error, Limits, Meter, Name, ObjectId, Position, Result, Value};
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

/// Replace independently balanced Tx sections, preserving every other byte.
/// The compatibility policy removes unmarked text only when Tx is absent.
pub(super) fn replace(
    document: &Document,
    resources: &Dictionary,
    bytes: &[u8],
    text: &[u8],
    policy: UpdatePolicy,
) -> Result<Vec<u8>> {
    let limits = document.limits();
    let mut tokens = Meter::new(limits.operations, "appearance update token budget exceeded");
    let mut storage = Meter::new(
        limits.document_bytes,
        "appearance update storage budget exceeded",
    );
    let spans = scan(
        document,
        resources,
        bytes,
        policy,
        &mut tokens,
        &mut storage,
    )?;
    let unmarked = spans.regions.is_empty() && policy == UpdatePolicy::PreserveExisting;
    if unmarked {
        if spans.clipping {
            return Err(Error::unsupported(
                features::APPEARANCE_UPDATE,
                "unmarked appearance contains inseparable text clipping",
            ));
        }
        let mut stack = Vec::new();
        let mut walk = Meter::new(
            limits.traversal_work,
            "appearance artwork traversal budget exceeded",
        );
        for name in &spans.xobjects {
            artwork(
                document,
                resources,
                name,
                &mut stack,
                &mut tokens,
                &mut storage,
                &mut walk,
            )?;
        }
    }
    let mut out = Program::new(&limits);
    if unmarked {
        out.text("q\n")?;
    }
    let mut copied = 0;
    for region in if unmarked {
        &spans.shown
    } else {
        &spans.regions
    } {
        out.bytes(slice(bytes, copied..region.start)?)?;
        if !unmarked {
            out.bytes(text)?;
        }
        copied = region.end;
    }
    out.bytes(slice(bytes, copied..bytes.len())?)?;
    if unmarked {
        out.text("\nQ\n")?;
    }
    if spans.regions.is_empty() {
        if !unmarked {
            out.text("\n")?;
        }
        out.bytes(text)?;
    }
    Ok(out.finish())
}

#[derive(Default)]
struct Spans {
    regions: Vec<Range<usize>>,
    shown: Vec<Range<usize>>,
    xobjects: Vec<Name>,
    clipping: bool,
}

/// The sole content walk for replacement and the compatibility text check.
/// Inline framing is shared with ordinary interpretation, including named
/// colour spaces and codec-delimited data. No samples are painted here.
fn scan(
    document: &Document,
    resources: &Dictionary,
    bytes: &[u8],
    policy: UpdatePolicy,
    tokens: &mut Meter,
    storage: &mut Meter,
) -> Result<Spans> {
    let limits = document.limits();
    if bytes.len() > limits.decoded_bytes {
        return Err(Error::limit("existing appearance byte budget exceeded"));
    }
    let content = Content::from_bytes(bytes);
    let mut reader = Reader::new(&content, &limits);
    let mut sections = Sections::default();
    let mut spans = Spans::default();
    while let Some(operation) = reader.next(tokens)? {
        let position = Position::Buffer(operation.offset);
        if let Some(region) = sections
            .step(&operation.op, operation.range.clone(), &limits)
            .map_err(|error| error.at(position))?
        {
            storage.reserve_capacity(&mut spans.regions, 1)?;
            spans.regions.push(region);
        }
        match operation.op {
            Op::InlineImage(dictionary) => {
                reader.inline_image_data(|data| {
                    pagura_image::inline_data_length_with_resources(
                        document,
                        resources,
                        &dictionary,
                        data,
                    )
                })?;
            }
            Op::ShowText(_)
            | Op::ShowTextAdjusted(_)
            | Op::NextLineShowText(_)
            | Op::NextLineShowTextSpaced { .. }
                if policy == UpdatePolicy::PreserveExisting =>
            {
                storage.reserve_capacity(&mut spans.shown, 1)?;
                spans.shown.push(operation.range);
            }
            Op::XObject(name) if policy == UpdatePolicy::PreserveExisting => {
                storage.charge(name.heap_bytes())?;
                storage.reserve_capacity(&mut spans.xobjects, 1)?;
                spans.xobjects.push(name);
            }
            Op::RenderMode(mode) => spans.clipping |= !(0..=3).contains(&mode),
            _ => (),
        }
    }
    if !reader.is_balanced() || !sections.balanced() {
        return Err(
            Error::syntax("unfinished appearance content").at(content.position(content.end()))
        );
    }
    Ok(spans)
}

/// Preserve Image XObjects and Forms whose whole reachable program is text-free.
/// Forms containing text have no reliable field/artwork distinction, so the
/// unmarked compatibility fallback refuses them instead of rewriting resources.
fn artwork(
    document: &Document,
    resources: &Dictionary,
    name: &Name,
    stack: &mut Vec<ObjectId>,
    tokens: &mut Meter,
    storage: &mut Meter,
    walk: &mut Meter,
) -> Result<()> {
    walk.charge(stack.len().saturating_add(1))?;
    let value = resources
        .get(b"XObject")
        .map(|value| document.resolve_dictionary(value))
        .transpose()?
        .and_then(|objects| objects.get(name.as_bytes()))
        .ok_or(Error::syntax("undefined appearance XObject"))?;
    let id = value.as_reference().ok_or(Error::syntax(
        "appearance XObject must be a stream reference",
    ))?;
    let object = document.object(id)?;
    let dictionary = object
        .dictionary()
        .filter(|_| object.is_stream())
        .ok_or(Error::syntax("appearance XObject must be a stream").in_object(id))?;
    if dictionary.is_name(b"Subtype", b"Image") {
        return Ok(());
    }
    if !dictionary.is_name(b"Subtype", b"Form") {
        return Err(Error::unsupported(
            features::APPEARANCE_UPDATE,
            "unidentified appearance XObject",
        )
        .in_object(id));
    }
    if stack.contains(&id) {
        return Err(Error::cycle("recursive appearance Form artwork").in_object(id));
    }
    if stack.len() >= document.limits().depth {
        return Err(Error::limit("appearance Form artwork depth exceeded").in_object(id));
    }
    storage.reserve_capacity(stack, 1)?;
    stack.push(id);
    let resources = dictionary
        .get(b"Resources")
        .map(|value| document.resolve_dictionary(value))
        .transpose()?
        .unwrap_or(resources);
    let before = storage.used();
    let spans = scan(
        document,
        resources,
        &document.stream_data(object)?,
        UpdatePolicy::PreserveExisting,
        tokens,
        storage,
    )
    .map_err(|error| error.in_object(id))?;
    let retained = storage.used() - before;
    if !spans.shown.is_empty() || spans.clipping {
        return Err(Error::unsupported(
            features::APPEARANCE_UPDATE,
            "unmarked Form artwork contains inseparable text",
        )
        .in_object(id));
    }
    for name in &spans.xobjects {
        artwork(document, resources, name, stack, tokens, storage, walk)?;
    }
    drop(spans);
    storage.refund(retained);
    stack.pop();
    Ok(())
}

fn slice(bytes: &[u8], range: Range<usize>) -> Result<&[u8]> {
    bytes
        .get(range)
        .ok_or(Error::syntax("invalid appearance content span"))
}

/// Scope state used only to decide whether a Tx span can be replaced intact.
#[derive(Default)]
struct Sections {
    graphics: usize,
    text: bool,
    depth: usize,
    active: Option<Start>,
}

struct Start {
    offset: usize,
    depth: usize,
    graphics: usize,
}

impl Sections {
    fn balanced(&self) -> bool {
        self.graphics == 0 && !self.text && self.depth == 0
    }

    fn step(
        &mut self,
        op: &Op<'_>,
        range: Range<usize>,
        limits: &Limits,
    ) -> Result<Option<Range<usize>>> {
        match op {
            Op::BeginText => {
                if self.text {
                    return Err(Error::syntax("nested appearance text object"));
                }
                self.text = true;
            }
            Op::EndText => {
                if !self.text {
                    return Err(Error::syntax("appearance ET without BT"));
                }
                self.text = false;
            }
            Op::Save => {
                self.graphics = self
                    .graphics
                    .checked_add(1)
                    .filter(|&n| n <= limits.depth)
                    .ok_or(Error::limit("appearance graphics stack budget exceeded"))?;
            }
            Op::Restore => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|start| self.graphics <= start.graphics)
                {
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
            Op::BeginMarkedContent(tag) | Op::BeginMarkedContentProperties { tag, .. } => {
                if tag == b"Tx" && self.active.is_none() {
                    if self.text {
                        return Err(Error::unsupported(
                            features::APPEARANCE_UPDATE,
                            "variable text section inside a text object is unavailable",
                        ));
                    }
                    self.active = Some(Start {
                        offset: range.start,
                        depth: self.depth,
                        graphics: self.graphics,
                    });
                }
                self.depth = self
                    .depth
                    .checked_add(1)
                    .filter(|&n| n <= limits.depth)
                    .ok_or(Error::limit("appearance marked-content depth exceeded"))?;
            }
            Op::EndMarkedContent => {
                self.depth = self
                    .depth
                    .checked_sub(1)
                    .ok_or(Error::syntax("appearance EMC without BMC"))?;
                if let Some(start) = &self.active
                    && start.depth == self.depth
                {
                    if self.text || self.graphics != start.graphics {
                        return Err(Error::unsupported(
                            features::APPEARANCE_UPDATE,
                            "variable text section crosses a graphics or text scope",
                        ));
                    }
                    let region = start.offset..range.end;
                    self.active = None;
                    return Ok(Some(region));
                }
            }
            _ => (),
        }
        Ok(None)
    }
}
