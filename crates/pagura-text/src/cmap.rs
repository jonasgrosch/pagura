//! Bounded declarative CMaps, ISO 32000-2 9.7.5–9.7.6 and 9.10.3.
use crate::{
    GlyphText, WritingMode,
    cid::Cid,
    cid::map::{CidMap, CidRange},
    features,
    report::Report,
};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Error, Limits, Meter, Name, ObjectId, Result, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;
use std::sync::{Arc, LazyLock};

mod block;
mod tolerate;
mod unicode;
use block::{Block, Target};
use unicode::{Lookup, UnicodeMap};

/// The most entries one CMap block may declare (9.7.5.4; TN5014 7.4).
const MAX_BLOCK_ENTRIES: usize = 100;

/// What a CMap maps codes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CMapRole {
    /// Character codes to CIDs: a Type 0 font's `/Encoding` (9.7.5).
    Encoding,
    /// Character codes to Unicode: a font's `/ToUnicode` (9.10.3).
    ToUnicode,
}

/// Budgets shared by a CMap and every stream it inherits from (ADR 0016).
pub(crate) struct Budget {
    work: Meter,
    entries: Meter,
    /// Decoded CMap stream bytes along the `UseCMap` chain.
    bytes: Meter,
    /// Unicode destination bytes held by the maps.
    text: Meter,
}

impl Budget {
    pub(crate) const fn new(limits: &Limits) -> Self {
        Self {
            work: Meter::new(limits.font_work, "CMap work budget exceeded"),
            entries: Meter::new(limits.font_entries, "CMap entry budget exceeded"),
            bytes: Meter::new(limits.decoded_bytes, "CMap stream byte budget exceeded"),
            text: Meter::new(limits.decoded_bytes, "CMap Unicode byte budget exceeded"),
        }
    }
}

struct ParseContext<'a, 'b> {
    role: CMapRole,
    budget: &'b mut Budget,
    report: Report<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Code {
    pub(crate) len: u8,
    pub(crate) value: u32,
}

impl Code {
    fn parse(value: &Value) -> Result<Self> {
        let bytes = value
            .as_string()
            .ok_or(Error::syntax("CMap code is not a string"))?;
        let len = u8::try_from(bytes.len()).map_err(|_| Error::syntax("CMap code too long"))?;
        if !(1..=4).contains(&len) {
            return Err(Error::syntax("CMap code length outside 1 to 4"));
        }
        Ok(Self {
            value: bytes.iter().fold(0, |v, b| (v << 8) | u32::from(*b)),
            len,
        })
    }

    fn of(bytes: &[u8]) -> Option<Self> {
        Some(Self {
            value: bytes.iter().fold(0, |v, b| (v << 8) | u32::from(*b)),
            len: u8::try_from(bytes.len()).ok()?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Range {
    first: Code,
    last: Code,
}

impl Range {
    fn new(first: Code, last: Code) -> Result<Self> {
        if first.len != last.len || first.value > last.value {
            return Err(Error::syntax("invalid CMap range"));
        }
        Ok(Self { first, last })
    }

    fn prefix_overlaps(&self, other: &Self) -> bool {
        let shared = usize::from(self.first.len.min(other.first.len));
        (0..shared).all(|index| {
            let shift_a = 8 * (usize::from(self.first.len) - 1 - index);
            let shift_b = 8 * (usize::from(other.first.len) - 1 - index);
            let lo_a = (self.first.value >> shift_a) & 255;
            let hi_a = (self.last.value >> shift_a) & 255;
            let lo_b = (other.first.value >> shift_b) & 255;
            let hi_b = (other.last.value >> shift_b) & 255;
            lo_a <= hi_b && lo_b <= hi_a
        })
    }

    // Codespaces compare each byte, not the integer interval (9.7.6.2).
    fn prefix(&self, bytes: &[u8]) -> bool {
        bytes.len() <= usize::from(self.first.len)
            && bytes.iter().enumerate().all(|(i, b)| {
                let shift = 8 * (usize::from(self.first.len) - 1 - i);
                let lo = (self.first.value >> shift) & 255;
                let hi = (self.last.value >> shift) & 255;
                (lo..=hi).contains(&u32::from(*b))
            })
    }
}

/// Codespace ranges grouped by code length, so decoding a code compares
/// each prefix only with the ranges of that length (9.7.6.2).
#[derive(Debug, Clone, Default)]
struct Codespaces([Vec<Range>; 4]);

impl Codespaces {
    fn push(&mut self, range: Range) {
        if let Some(group) = self.0.get_mut(usize::from(range.first.len) - 1) {
            group.push(range);
        }
    }

    fn of_len(&self, len: u8) -> &[Range] {
        usize::from(len)
            .checked_sub(1)
            .and_then(|index| self.0.get(index))
            .map_or(&[], Vec::as_slice)
    }

    fn iter(&self) -> impl Iterator<Item = &Range> {
        self.0.iter().flatten()
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(Vec::is_empty)
    }

    fn heap_bytes(&self) -> usize {
        self.0.iter().fold(0_usize, |sum, group| {
            sum.saturating_add(group.capacity().saturating_mul(size_of::<Range>()))
        })
    }
}

/// A parsed CMap. Inherited mappings (`usecmap`) stay in a shared parent
/// that lookups consult after this map's own definitions, so inheriting a
/// large predefined CMap copies nothing.
#[derive(Debug, Default)]
pub(crate) struct CMap {
    spaces: Codespaces,
    mappings: CidMap,
    notdef: CidMap,
    unicode: UnicodeMap,
    pub(crate) mode: WritingMode,
    mode_explicit: bool,
    use_name: Option<Name>,
    registry: Option<Box<[u8]>>,
    ordering: Option<Box<[u8]>>,
    parent: Option<Parent>,
}

#[derive(Debug)]
struct Parent {
    map: Arc<CMap>,
    /// Provider-owned or static, so excluded from font storage estimates.
    shared: bool,
}

static IDENTITY_H: LazyLock<Arc<CMap>> =
    LazyLock::new(|| Arc::new(CMap::identity(WritingMode::Horizontal)));
static IDENTITY_V: LazyLock<Arc<CMap>> =
    LazyLock::new(|| Arc::new(CMap::identity(WritingMode::Vertical)));

impl CMap {
    /// The CIDs reachable through this map, including explicit and implicit
    /// notdef mappings and inherited ones; CIDs may repeat.
    pub(crate) fn cids(&self) -> Box<dyn Iterator<Item = Cid> + '_> {
        let own = self.mappings.cids().chain(self.notdef.cids());
        match &self.parent {
            Some(parent) => Box::new(own.chain(parent.map.cids())),
            None => Box::new(own.chain(std::iter::once(Cid::NOTDEF))),
        }
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        let names = self
            .use_name
            .as_ref()
            .map_or(0, Name::len)
            .saturating_add(self.registry.as_ref().map_or(0, |r| r.len()))
            .saturating_add(self.ordering.as_ref().map_or(0, |o| o.len()));
        let parent = match &self.parent {
            Some(Parent { map, shared: false }) => size_of::<Self>()
                .saturating_add(2 * size_of::<usize>())
                .saturating_add(map.heap_bytes()),
            Some(Parent { shared: true, .. }) | None => 0,
        };
        self.spaces
            .heap_bytes()
            .saturating_add(self.mappings.heap_bytes())
            .saturating_add(self.notdef.heap_bytes())
            .saturating_add(self.unicode.heap_bytes())
            .saturating_add(names)
            .saturating_add(parent)
    }

    fn identity(mode: WritingMode) -> Self {
        let mut spaces = Codespaces::default();
        spaces.push(Range {
            first: Code { value: 0, len: 2 },
            last: Code {
                value: u32::from(u16::MAX),
                len: 2,
            },
        });
        Self {
            spaces,
            mappings: CidMap::identity(),
            mode,
            ..Self::default()
        }
    }

    /// The `/CIDSystemInfo` registry, when declared or inherited.
    pub(crate) fn registry(&self) -> Option<&[u8]> {
        self.registry.as_deref()
    }

    /// The `/CIDSystemInfo` ordering, when declared or inherited.
    pub(crate) fn ordering(&self) -> Option<&[u8]> {
        self.ordering.as_deref()
    }

    /// The registry and ordering, when both are declared or inherited.
    pub(crate) fn collection(&self) -> Option<(&[u8], &[u8])> {
        Some((self.registry()?, self.ordering()?))
    }

    pub(crate) fn use_name(&self) -> Option<&Name> {
        self.use_name.as_ref()
    }

    /// Parses a standalone CMap; every repair is rejected.
    pub(crate) fn parse(
        data: &[u8],
        origin: Origin,
        limits: &Limits,
        role: CMapRole,
    ) -> Result<Self> {
        Self::parse_reported(
            data,
            origin,
            limits,
            role,
            Report::strict(),
            &mut Budget::new(limits),
        )
    }

    fn parse_reported(
        data: &[u8],
        origin: Origin,
        limits: &Limits,
        role: CMapRole,
        report: Report<'_>,
        budget: &mut Budget,
    ) -> Result<Self> {
        let mut context = ParseContext {
            role,
            budget,
            report,
        };
        let mut parser = Parser::new(data, origin, *limits);
        let mut map = Self::default();
        let mut operands = Vec::new();
        while let Some(byte) = parser.peek() {
            context.budget.work.charge(1)?;
            if matches!(
                byte,
                b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.' | b'0'..=b'9'
            ) {
                if operands.len() >= limits.font_entries {
                    return Err(Error::limit("CMap operand limit exceeded").at(parser.position()));
                }
                operands.push(parser.parse()?);
                continue;
            }
            let position = parser.position();
            let operator = parser.word()?;
            let result = if let Some(block) = Block::parse(operator) {
                map.block(&mut parser, block, &mut operands, &mut context, limits)
            } else {
                match operator {
                    b"def" => map.definition(&operands),
                    b"usecmap" => operands
                        .last()
                        .and_then(Value::as_name)
                        .ok_or(Error::syntax("usecmap requires a name"))
                        .map(|name| map.use_name = Some(name.clone())),
                    b"usefont" => {
                        if operands.last().and_then(Value::as_integer) == Some(0) {
                            Ok(())
                        } else {
                            Err(Error::syntax("PDF CMap font number must be zero"))
                        }
                    }
                    // Declarative resource wrapper only: never execute PostScript.
                    b"begincmap" | b"endcmap" | b"dict" | b"begin" | b"end" | b"findresource"
                    | b"defineresource" | b"pop" | b"currentdict" | b"CMapName" => Ok(()),
                    b"dup" | b"readonly" => continue,
                    b"beginrearrangedfont"
                    | b"endrearrangedfont"
                    | b"beginusematrix"
                    | b"endusematrix" => Err(Error::syntax("CMap operator is forbidden in PDF")),
                    _ => Err(Error::unsupported(
                        features::CMAP_PROGRAM,
                        "CMap requires unsupported PostScript evaluation",
                    )),
                }
            };
            result.map_err(|e| e.at(position))?;
            operands.clear();
        }
        if !operands.is_empty() {
            return Err(Error::syntax("unterminated CMap operands").at(parser.position()));
        }
        Ok(map)
    }

    fn block(
        &mut self,
        parser: &mut Parser<'_>,
        block: Block,
        operands: &mut Vec<Value>,
        context: &mut ParseContext<'_, '_>,
        limits: &Limits,
    ) -> Result<()> {
        let position = parser.position();
        let count = operands
            .pop()
            .and_then(|v| v.as_integer())
            .and_then(|v| usize::try_from(v).ok())
            .ok_or(Error::syntax("invalid CMap block count"))?;
        if count > MAX_BLOCK_ENTRIES {
            context.report.repair(
                crate::repairs::CMAP_BLOCK_COUNT,
                "CMap block count exceeds 100 (9.7.5.4; TN5014 7.4)",
                position,
            )?;
        }
        if count > limits.font_entries || count > limits.font_work {
            return Err(Error::limit("CMap block entry budget exceeded"));
        }
        for _ in 0..count {
            context.budget.work.charge(1)?;
            self.entry(parser, block, context)
                .map_err(|e| e.at(parser.position()))?;
        }
        if parser.keyword(block.end()) {
            Ok(())
        } else {
            Err(Error::syntax("CMap block count or terminator mismatch"))
        }
    }

    fn entry(
        &mut self,
        parser: &mut Parser<'_>,
        block: Block,
        context: &mut ParseContext<'_, '_>,
    ) -> Result<()> {
        let first = Code::parse(&parser.parse()?)?;
        let last = if block.is_range() {
            Code::parse(&parser.parse()?)?
        } else {
            first
        };
        let range = Range::new(first, last)?;
        match block.target() {
            Target::Codespace => self.codespace(range, context, parser.position()),
            Target::Unicode => {
                let value = parser.parse()?;
                if context.role != CMapRole::ToUnicode {
                    return Err(Error::syntax("Unicode mappings in an Encoding CMap"));
                }
                self.unicode_range(range, &value, context, parser.position())
            }
            Target::Cid(step) => {
                let value = parser.parse()?;
                if context.role != CMapRole::Encoding {
                    return Err(Error::syntax("CID mappings in a ToUnicode CMap"));
                }
                let cid = value
                    .as_integer()
                    .and_then(Cid::from_integer)
                    .ok_or(Error::syntax("CMap CID outside 16-bit range"))?;
                let range = CidRange::new(first, last, cid, step)?;
                context.budget.entries.charge(1)?;
                let target = match step {
                    crate::cid::map::Step::Increment => &mut self.mappings,
                    crate::cid::map::Step::Constant => &mut self.notdef,
                };
                target.insert(range, &mut context.budget.work)
            }
        }
    }

    /// `/Registry`, `/Ordering` (inside `/CIDSystemInfo`) and `/WMode`.
    fn definition(&mut self, operands: &[Value]) -> Result<()> {
        match operands {
            [.., Value::Name(key), Value::String(value)] if key == b"Registry" => {
                self.registry = Some(value.as_slice().into());
            }
            [.., Value::Name(key), Value::String(value)] if key == b"Ordering" => {
                self.ordering = Some(value.as_slice().into());
            }
            [.., Value::Name(key), value] if key == b"WMode" => {
                self.mode = mode(value.as_integer())?;
                self.mode_explicit = true;
            }
            _ => {}
        }
        Ok(())
    }

    /// Inherits a parent CMap (`usecmap`). Lookups consult it after this
    /// map's own definitions; collection and writing mode are copied.
    fn inherit(&mut self, parent: Arc<Self>, shared: bool) {
        if self.registry.is_none() {
            self.registry.clone_from(&parent.registry);
        }
        if self.ordering.is_none() {
            self.ordering.clone_from(&parent.ordering);
        }
        if !self.mode_explicit {
            self.mode = parent.mode;
        }
        self.parent = Some(Parent {
            map: parent,
            shared,
        });
    }

    // TN5014 5.4: using resources cannot redefine their inherited codespace.
    pub(crate) fn inherit_resource(&mut self, parent: Arc<Self>) -> Result<()> {
        if !self.spaces.is_empty() {
            return Err(Error::syntax(
                "predefined resource redefines inherited codespace",
            ));
        }
        self.inherit(parent, true);
        Ok(())
    }

    /// The codespace in effect: this map's own, else the inherited one.
    fn spaces(&self) -> &Codespaces {
        match &self.parent {
            Some(parent) if self.spaces.is_empty() => parent.map.spaces(),
            _ => &self.spaces,
        }
    }

    pub(crate) fn validate(&self, work: &mut Meter) -> Result<()> {
        let spaces: Vec<&Range> = self.spaces.iter().collect();
        for (i, a) in spaces.iter().enumerate() {
            for b in &spaces[..i] {
                work.charge(1)?;
                if a.prefix_overlaps(b) {
                    return Err(Error::syntax("overlapping or ambiguous CMap codespaces"));
                }
            }
        }
        if self.spaces().is_empty() {
            return Err(Error::syntax("CMap has no codespace"));
        }
        Ok(())
    }

    fn layers(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |map| {
            map.parent.as_ref().map(|parent| parent.map.as_ref())
        })
    }

    fn simple_codes(&self) -> bool {
        self.spaces().iter().all(|range| range.first.len == 1)
            && self
                .layers()
                .all(|map| map.unicode.codes().all(|(code, _)| code.len == 1))
    }

    fn has_long_unicode_codes(&self) -> bool {
        self.layers()
            .any(|map| map.unicode.codes().any(|(code, _)| code.len > 2))
    }

    pub(crate) fn consistent_with(&self, encoding: &Self, limits: &Limits) -> Result<()> {
        let mut work = Meter::new(limits.font_work, "ToUnicode check work budget exceeded");
        for unicode in self.spaces().iter() {
            for font in encoding.spaces().iter() {
                work.charge(1)?;
                if unicode.first.len != font.first.len && unicode.prefix_overlaps(font) {
                    return Err(Error::syntax(
                        "ToUnicode code length conflicts with font Encoding",
                    ));
                }
            }
        }
        Ok(())
    }

    /// The next code of `input` (9.7.6.2), and whether it lies in the
    /// codespace. Undefined input consumes the shortest range with the
    /// longest matching prefix.
    pub(crate) fn next(&self, input: &[u8], work: &mut Meter) -> Result<(Code, bool)> {
        let spaces = self.spaces();
        for len in 1..=4_u8 {
            let Some(prefix) = input.get(..usize::from(len)) else {
                break;
            };
            let candidates = spaces.of_len(len);
            work.charge(candidates.len())?;
            if candidates.iter().any(|range| range.prefix(prefix))
                && let Some(code) = Code::of(prefix)
            {
                return Ok((code, true));
            }
        }
        let mut best: Option<(usize, usize)> = None;
        for range in spaces.iter() {
            work.charge(4)?;
            let len = usize::from(range.first.len);
            let matched = (1..=len.min(input.len()))
                .take_while(|n| range.prefix(&input[..*n]))
                .count();
            if best.is_none_or(|(most, shortest)| {
                matched > most || (matched == most && len < shortest)
            }) {
                best = Some((matched, len));
            }
        }
        let (_, len) = best.ok_or(Error::syntax("CMap has no codespace"))?;
        let code =
            Code::of(&input[..input.len().min(len)]).ok_or(Error::syntax("invalid code length"))?;
        Ok((code, false))
    }

    pub(crate) fn cid(&self, code: Code, work: &mut Meter) -> Result<Option<Cid>> {
        for map in self.layers() {
            if let Some(cid) = map.mappings.get(code, work)? {
                return Ok(Some(cid));
            }
        }
        Ok(None)
    }

    pub(crate) fn notdef(&self, code: Code, work: &mut Meter) -> Result<Cid> {
        for map in self.layers() {
            if let Some(cid) = map.notdef.get(code, work)? {
                return Ok(cid);
            }
        }
        Ok(Cid::NOTDEF)
    }

    pub(crate) fn text(&self, code: Code) -> Option<GlyphText> {
        for map in self.layers() {
            match map.unicode.get(code) {
                Lookup::Text(text) => return Some(text),
                Lookup::Absent => return None,
                Lookup::Missing => {}
            }
        }
        None
    }
}

fn mode(value: Option<i64>) -> Result<WritingMode> {
    match value {
        Some(0) => Ok(WritingMode::Horizontal),
        Some(1) => Ok(WritingMode::Vertical),
        _ => Err(Error::syntax("invalid CMap WMode")),
    }
}

/// Loads a CMap: a predefined name or an embedded stream with its
/// `UseCMap` chain. Predefined maps come from the provider (or are the
/// static Identity-H/V maps) and are shared, never copied.
pub(crate) fn load(
    document: &Document,
    value: &Value,
    role: CMapRole,
    provider: Option<&dyn crate::Provider>,
    report: Report<'_>,
) -> ReadResult<Arc<CMap>> {
    load_inner(
        document,
        value,
        role,
        provider,
        &mut Vec::new(),
        &mut Budget::new(&document.limits()),
        report,
    )
}

fn load_inner(
    document: &Document,
    value: &Value,
    role: CMapRole,
    provider: Option<&dyn crate::Provider>,
    path: &mut Vec<ObjectId>,
    budget: &mut Budget,
    report: Report<'_>,
) -> ReadResult<Arc<CMap>> {
    if let Some(name) = document.resolve(value)?.as_name() {
        return Ok(predefined(name, role, provider)?);
    }
    let id = value
        .as_reference()
        .ok_or(Error::syntax("CMap must be a name or stream reference"))?;
    if path.contains(&id) {
        return Err(Error::cycle("UseCMap cycle").in_object(id).into());
    }
    let limits = document.limits();
    if path.len() >= limits.depth {
        return Err(Error::limit("UseCMap depth exceeded").in_object(id).into());
    }
    path.push(id);
    let object = document.object(id)?;
    let dict = object
        .dictionary()
        .ok_or(Error::syntax("CMap is not a stream"))?;
    let data = document.stream_data(object)?;
    budget
        .bytes
        .charge(data.len())
        .map_err(|e| e.in_object(id))?;
    let mut map = CMap::parse_reported(&data, Origin::Stream(id), &limits, role, report, budget)
        .map_err(|e| e.in_object(id))?;
    if role == CMapRole::Encoding
        && let Some(value) = dict.get(b"WMode")
    {
        let declared = mode(document.resolve(value)?.as_integer())?;
        if map.mode_explicit && map.mode != declared {
            return Err(Error::syntax("CMap WMode disagrees with stream dictionary")
                .in_object(id)
                .into());
        }
        map.mode = declared;
        map.mode_explicit = true;
    }
    if let Some(parent) = dict.get(b"UseCMap") {
        let resolved = document.resolve(parent)?;
        if let Some(name) = &map.use_name {
            let parent_name = if let Some(name) = resolved.as_name() {
                Some(name)
            } else if role == CMapRole::ToUnicode {
                None
            } else {
                document.resolve_dictionary(parent)?.get_name(b"CMapName")
            };
            if (role == CMapRole::Encoding || resolved.as_name().is_some())
                && parent_name != Some(name)
            {
                return Err(Error::syntax("usecmap disagrees with UseCMap")
                    .in_object(id)
                    .into());
            }
        }
        let shared = resolved.as_name().is_some();
        let parent = load_inner(document, parent, role, provider, path, budget, report)?;
        map.inherit(parent, shared);
    } else if map.use_name.is_some() {
        return Err(Error::syntax("usecmap missing UseCMap dictionary entry")
            .in_object(id)
            .into());
    }
    map.validate(&mut budget.work)
        .map_err(|e| e.in_object(id))?;
    path.pop();
    Ok(Arc::new(map))
}

/// A predefined CMap by name (9.7.5.2, Table 116).
fn predefined(
    name: &Name,
    role: CMapRole,
    provider: Option<&dyn crate::Provider>,
) -> Result<Arc<CMap>> {
    if role == CMapRole::ToUnicode {
        return Err(Error::unsupported(
            features::PREDEFINED_CMAP,
            "predefined ToUnicode CMap data is not bundled",
        ));
    }
    match name.as_bytes() {
        b"Identity-H" => return Ok(Arc::clone(&IDENTITY_H)),
        b"Identity-V" => return Ok(Arc::clone(&IDENTITY_V)),
        _ => {}
    }
    if let Some(provider) = provider
        && let Some(map) = provider.predefined_cmap(name.as_bytes())?
    {
        return Ok(Arc::clone(&map.0));
    }
    Err(Error::unsupported(
        features::PREDEFINED_CMAP,
        "predefined CMap data is not bundled",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::{document, expected, integer, rows, text};
    use pagura_core::ObjectId;
    use pagura_testkit::fixture;

    fn parse(data: &[u8], limits: &Limits, role: CMapRole) -> Result<CMap> {
        CMap::parse(data, Origin::Buffer, limits, role)
    }

    #[test]
    fn malformed_blocks_and_utf16_are_rejected() {
        for data in [
            b"1 begincidchar <01> 4 endcidrange".as_slice(),
            b"1 begincodespacerange <00> <ffff> endcodespacerange",
            b"1 begincodespacerange <81ff> <8200> endcodespacerange",
            b"1 beginbfrange <00> <01> [<0041>] endbfrange",
            b"1 beginbfchar <00> <D800> endbfchar",
            b"1 beginbfchar <00> <00> endbfchar",
            b"1 beginbfrange <00> <01> <00ff> endbfrange",
            b"101 begincidchar",
            b"1 usefont",
            b"1 begincidrange <00000000> <ffffffff> 1 endcidrange",
            b"1 beginrearrangedfont",
        ] {
            assert!(
                parse(data, &Limits::default(), CMapRole::ToUnicode).is_err(),
                "{data:?}"
            );
        }
    }

    #[test]
    fn huge_unicode_ranges_and_budgeted_work_fail_before_expansion() {
        let limits = Limits {
            font_work: 10,
            ..Limits::default()
        };
        assert!(
            parse(
                b"1 beginbfrange <00000000> <ffffffff> <0041> endbfrange",
                &limits,
                CMapRole::ToUnicode
            )
            .is_err()
        );
        let limits = Limits {
            decoded_bytes: 1,
            ..Limits::default()
        };
        assert!(
            parse(
                b"1 beginbfchar <00> <D83DDE00> endbfchar",
                &limits,
                CMapRole::ToUnicode
            )
            .is_err()
        );
    }

    #[test]
    fn a_million_code_bfrange_stores_runs_not_codes() -> ReadResult<()> {
        // One 4-byte range over a million codes keeps three intervals: valid
        // destinations, surrogates (no text), valid again until the 2-byte
        // destination overflows and the range ends (9.10.3).
        let document = Document::open(
            fixture("text", "type3-differences.pdf"),
            &pagura_document::OpenOptions::default(),
        )?;
        let limits = document.limits();
        let map = pagura_document::DiagnosticBatch::run(&document, |batch| {
            Ok(CMap::parse_reported(
                b"1 begincodespacerange <00000000> <ffffffff> endcodespacerange \
              1 beginbfrange <00000000> <000F423F> <0041> endbfrange",
                Origin::Buffer,
                &limits,
                CMapRole::ToUnicode,
                Report::new(batch),
                &mut Budget::new(&limits),
            )?)
        })?;
        assert_eq!(map.unicode.len(), 3);
        assert!(map.heap_bytes() < 1024);
        let code = |value| Code { value, len: 4 };
        for (value, text) in [
            (0, Some("A")),
            (1, Some("B")),
            (0xd7be, Some("\u{d7ff}")),
            (0xd7bf, None),
            (0xdfbe, None),
            (0xdfbf, Some("\u{e000}")),
            (0xffbe, Some("\u{ffff}")),
            (0xffbf, None),
            (0x0f_423f, None),
        ] {
            assert_eq!(map.text(code(value)).as_deref(), text, "{value:x}");
        }
        Ok(())
    }

    #[test]
    fn identity_lengths_and_word_spacing() -> Result<()> {
        let map = CMap::identity(WritingMode::Horizontal);
        let mut work = Meter::new(usize::MAX, "test");
        let (code, valid) = map.next(b"\0 ", &mut work)?;
        assert!(valid);
        assert_eq!(code, Code { value: 32, len: 2 });
        let (code, valid) = map.next(b"\0", &mut work)?;
        assert!(!valid);
        assert_eq!(code.len, 1);
        assert!(Arc::ptr_eq(&IDENTITY_H, &IDENTITY_H.clone()));
        Ok(())
    }

    #[test]
    fn usecmap_errors_keep_their_position() {
        let error = parse(b"  42 usecmap", &Limits::default(), CMapRole::Encoding)
            .err()
            .map(|e| (e.message, e.position));
        assert_eq!(
            error,
            Some(("usecmap requires a name", pagura_core::Position::Buffer(5)))
        );
    }
    #[test]
    fn cmap_vectors() -> ReadResult<()> {
        for name in [
            "cmap-mixed",
            "cmap-bytewise",
            "cmap-usecmap",
            "cmap-parent-stream",
            "cmap-indirect-name",
            "unicode-parent",
            "unicode-ignored-stream-metadata",
            "unicode-inherited-ignored-name",
        ] {
            let document = document(name)?;
            let role = if name.starts_with("unicode-") {
                CMapRole::ToUnicode
            } else {
                CMapRole::Encoding
            };
            let map = pagura_document::DiagnosticBatch::run(&document, |batch| {
                load(
                    &document,
                    &Value::Reference(ObjectId::new(4, 0)),
                    role,
                    None,
                    Report::new(batch),
                )
            })?;
            let input = fixture("text", &format!("{name}.input"));
            let mut offset = 0;
            let mut work = Meter::new(usize::MAX, "test");
            for value in expected(name)? {
                let row = rows(&value)?;
                let (code, valid) = map.next(&input[offset..], &mut work)?;
                assert_eq!(code.value, integer(&row[0])?, "{name}");
                assert_eq!(u32::from(code.len), integer(&row[1])?, "{name}");
                let cid = if valid {
                    map.cid(code, &mut work)?.map(u32::from)
                } else {
                    None
                };
                assert_eq!(
                    cid,
                    if row[2].is_null() {
                        None
                    } else {
                        Some(integer(&row[2])?)
                    },
                    "{name}"
                );
                assert_eq!(u32::from(map.notdef(code, &mut work)?), integer(&row[3])?);
                assert_eq!(map.text(code).as_deref(), text(&row[4]));
                offset += usize::from(code.len);
            }
            assert_eq!(offset, input.len());
            assert_eq!(
                map.mode,
                if matches!(name, "cmap-mixed" | "cmap-usecmap" | "cmap-indirect-name") {
                    WritingMode::Vertical
                } else {
                    WritingMode::Horizontal
                }
            );
        }
        Ok(())
    }
    #[test]
    fn embedded_usecmap_chains_stop_at_the_depth_limit() -> ReadResult<()> {
        // Stream 4 inherits stream 5: a chain of two needs a depth of two.
        for (depth, limited) in [(1, true), (2, false)] {
            let document = Document::open(
                fixture("text", "cmap-parent-stream.pdf"),
                &pagura_document::OpenOptions {
                    limits: Limits {
                        depth,
                        ..Limits::default()
                    },
                    ..pagura_document::OpenOptions::default()
                },
            )?;
            let result = pagura_document::DiagnosticBatch::run(&document, |batch| {
                load(
                    &document,
                    &Value::Reference(ObjectId::new(4, 0)),
                    CMapRole::Encoding,
                    None,
                    Report::new(batch),
                )
            });
            match result {
                Err(error) if limited => {
                    assert_eq!(
                        error.as_error().unwrap().kind,
                        pagura_core::ErrorKind::Limit
                    );
                    assert_eq!(error.as_error().unwrap().object, Some(ObjectId::new(5, 0)));
                }
                Ok(_) if !limited => {}
                other => panic!("depth {depth}: {other:?}"),
            }
        }
        Ok(())
    }
}
