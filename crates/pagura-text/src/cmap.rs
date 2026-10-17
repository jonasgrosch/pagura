//! Bounded declarative CMaps, ISO 32000-2 9.7.5–9.7.6 and 9.10.3.
use crate::{
    WritingMode,
    cidmap::{CidMap, CidRange},
    features,
};
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Error, Limits, Name, ObjectId, Result, Value};
use pagura_document::Document;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Code {
    pub(crate) len: u8,
    pub(crate) value: u32,
}

impl Code {
    fn parse(value: &Value) -> Result<Self> {
        let bytes = value
            .as_string()
            .ok_or_else(|| Error::syntax("CMap code is not a string"))?;
        let len = u8::try_from(bytes.len()).map_err(|_| Error::syntax("CMap code too long"))?;
        if !(1..=4).contains(&len) {
            return Err(Error::syntax("CMap code length outside 1 to 4"));
        }
        Ok(Self {
            value: bytes.iter().fold(0, |v, b| (v << 8) | u32::from(*b)),
            len,
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

#[derive(Debug, Clone, Default)]
pub(crate) struct CMap {
    spaces: Vec<Range>,
    mappings: CidMap,
    notdef: CidMap,
    unicode: BTreeMap<Code, String>,
    pub(crate) mode: WritingMode,
    pub(crate) use_name: Option<Name>,
    mode_explicit: bool,
    unicode_bytes: usize,
    parse_work: usize,
}

impl CMap {
    pub(crate) fn identity(name: &[u8]) -> Result<Self> {
        let mode = match name {
            b"Identity-H" => WritingMode::Horizontal,
            b"Identity-V" => WritingMode::Vertical,
            _ => {
                return Err(Error::unsupported(
                    features::PREDEFINED_CMAP,
                    "predefined CMap data is not bundled",
                ));
            }
        };
        let range = Range::new(
            Code { value: 0, len: 2 },
            Code {
                value: 65535,
                len: 2,
            },
        )?;
        let mut mappings = CidMap::default();
        mappings.insert(
            CidRange {
                first: range.first,
                last: range.last,
                cid: 0,
                increment: true,
            },
            &mut 0,
            Limits::default(),
        )?;
        Ok(Self {
            spaces: vec![range],
            mappings,
            mode,
            ..Self::default()
        })
    }

    pub(crate) fn parse(
        data: &[u8],
        origin: Origin,
        limits: Limits,
        unicode: bool,
    ) -> Result<Self> {
        let mut parser = Parser::new(data, origin, limits);
        let mut map = Self::default();
        let mut operands = Vec::new();
        let mut work = 0;
        while let Some(byte) = parser.peek() {
            charge(&mut work, 1, limits.operations)?;
            if matches!(
                byte,
                b'/' | b'(' | b'<' | b'[' | b'+' | b'-' | b'.' | b'0'..=b'9'
            ) {
                if operands.len() >= limits.value_nodes {
                    return Err(Error::limit("CMap operand limit exceeded").at(parser.position()));
                }
                operands.push(parser.parse()?);
                continue;
            }
            let position = parser.position();
            let operator = parser.word()?;
            let result = match operator {
                b"begincodespacerange"
                | b"begincidchar"
                | b"begincidrange"
                | b"beginnotdefchar"
                | b"beginnotdefrange"
                | b"beginbfchar"
                | b"beginbfrange" => {
                    let count = operands
                        .pop()
                        .and_then(|v| v.as_integer())
                        .ok_or_else(|| Error::syntax("CMap block missing count"))?;
                    if !(0..=100).contains(&count) {
                        return Err(Error::syntax("CMap block count outside 0 to 100").at(position));
                    }
                    for _ in 0..count {
                        charge(&mut work, 1, limits.operations)?;
                        map.entry(&mut parser, operator, unicode, limits, &mut work)
                            .map_err(|e| e.at(parser.position()))?;
                    }
                    let end = match operator {
                        b"begincodespacerange" => b"endcodespacerange".as_slice(),
                        b"begincidchar" => b"endcidchar",
                        b"begincidrange" => b"endcidrange",
                        b"beginnotdefchar" => b"endnotdefchar",
                        b"beginnotdefrange" => b"endnotdefrange",
                        b"beginbfchar" => b"endbfchar",
                        _ => b"endbfrange",
                    };
                    if parser.keyword(end) {
                        Ok(())
                    } else {
                        Err(Error::syntax("CMap block count or terminator mismatch"))
                    }
                }
                b"def" => self_definition(&mut map, &operands),
                b"usecmap" => {
                    map.use_name = Some(
                        operands
                            .last()
                            .and_then(Value::as_name)
                            .ok_or_else(|| Error::syntax("usecmap requires a name"))?
                            .clone(),
                    );
                    Ok(())
                }
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
                b"dup" | b"readonly" => {
                    continue;
                }
                b"beginrearrangedfont"
                | b"endrearrangedfont"
                | b"beginusematrix"
                | b"endusematrix" => Err(Error::syntax("CMap operator is forbidden in PDF")),
                _ => Err(Error::unsupported(
                    features::CMAP_PROGRAM,
                    "CMap requires unsupported PostScript evaluation",
                )),
            };
            result.map_err(|e| e.at(position))?;
            operands.clear();
        }
        if !operands.is_empty() {
            return Err(Error::syntax("unterminated CMap operands").at(parser.position()));
        }
        map.parse_work = work;
        Ok(map)
    }

    fn entry(
        &mut self,
        parser: &mut Parser<'_>,
        op: &[u8],
        unicode: bool,
        limits: Limits,
        work: &mut usize,
    ) -> Result<()> {
        let first = Code::parse(&parser.parse()?)?;
        let is_range = op.ends_with(b"range");
        let last = if is_range {
            Code::parse(&parser.parse()?)?
        } else {
            first
        };
        let range = Range::new(first, last)?;
        if op == b"begincodespacerange" {
            for i in 0..usize::from(first.len) {
                let shift = 8 * i;
                if ((first.value >> shift) & 255) > ((last.value >> shift) & 255) {
                    return Err(Error::syntax("invalid codespace byte interval"));
                }
            }
            if self.spaces.len() >= limits.value_nodes {
                return Err(Error::limit("CMap codespace limit exceeded"));
            }
            self.spaces.push(range);
            return Ok(());
        }
        let value = parser.parse()?;
        if op.starts_with(b"beginbf") {
            if !unicode {
                return Err(Error::syntax("Unicode mappings in an Encoding CMap"));
            }
            self.unicode_range(range, &value, limits, work)
        } else {
            if unicode {
                return Err(Error::syntax("CID mappings in a ToUnicode CMap"));
            }
            let cid = value
                .as_integer()
                .and_then(|v| u16::try_from(v).ok())
                .ok_or_else(|| Error::syntax("CMap CID outside 16-bit range"))?;
            let notdef = op.starts_with(b"beginnotdef");
            if !notdef && last.value - first.value > 65535 - u32::from(cid) {
                return Err(Error::syntax("CMap CID range overflows"));
            }
            let target = if notdef {
                &mut self.notdef
            } else {
                &mut self.mappings
            };
            if target.len() >= limits.value_nodes {
                return Err(Error::limit("CMap mapping limit exceeded"));
            }
            target.insert(
                CidRange {
                    first,
                    last,
                    cid: u32::from(cid),
                    increment: !notdef,
                },
                work,
                limits,
            )
        }
    }

    fn unicode_range(
        &mut self,
        range: Range,
        value: &Value,
        limits: Limits,
        work: &mut usize,
    ) -> Result<()> {
        let count = u64::from(range.last.value) - u64::from(range.first.value) + 1;
        let count = usize::try_from(count).map_err(|_| Error::limit("Unicode range too large"))?;
        charge(work, count, limits.operations)?;
        if count > limits.value_nodes.saturating_sub(self.unicode.len()) {
            return Err(Error::limit("Unicode mapping limit exceeded"));
        }
        if let Some(array) = value.as_array() {
            if array.len() != count {
                return Err(Error::syntax("bfrange array length mismatch"));
            }
            for (i, value) in array.iter().enumerate() {
                let offset =
                    u32::try_from(i).map_err(|_| Error::limit("Unicode range too large"))?;
                self.insert_text(
                    Code {
                        value: range.first.value + offset,
                        len: range.first.len,
                    },
                    value
                        .as_string()
                        .ok_or_else(|| Error::syntax("invalid Unicode destination"))?,
                    limits,
                )?;
            }
        } else {
            let mut bytes = value
                .as_string()
                .ok_or_else(|| Error::syntax("invalid Unicode destination"))?
                .to_vec();
            if bytes.is_empty() && count > 1 {
                return Err(Error::syntax("empty bfrange destination cannot increment"));
            }
            for code in range.first.value..=range.last.value {
                self.insert_text(
                    Code {
                        value: code,
                        len: range.first.len,
                    },
                    &bytes,
                    limits,
                )?;
                if code != range.last.value {
                    // 9.10.3 increments only the final byte, without carrying.
                    let last = bytes
                        .last_mut()
                        .ok_or_else(|| Error::syntax("empty Unicode destination"))?;
                    *last = last
                        .checked_add(1)
                        .ok_or_else(|| Error::syntax("bfrange destination final byte overflows"))?;
                }
            }
        }
        Ok(())
    }

    fn insert_text(&mut self, code: Code, bytes: &[u8], limits: Limits) -> Result<()> {
        if bytes.len() > 512 {
            return Err(Error::syntax("Unicode destination exceeds 512 bytes"));
        }
        let text = utf16(bytes)?;
        let old = self.unicode.get(&code).map_or(0, String::len);
        self.unicode_bytes = self.unicode_bytes.saturating_sub(old);
        charge(&mut self.unicode_bytes, text.len(), limits.decoded_bytes)?;
        self.unicode.insert(code, text);
        Ok(())
    }

    fn inherit(&mut self, mut parent: Self, limits: Limits, work: &mut usize) -> Result<()> {
        if self
            .mappings
            .len()
            .saturating_add(parent.mappings.len())
            .saturating_add(self.notdef.len())
            .saturating_add(parent.notdef.len())
            > limits.value_nodes
        {
            return Err(Error::limit("inherited CMap mapping limit exceeded"));
        }
        if self.spaces.is_empty() {
            self.spaces = parent.spaces;
        }
        if !self.mode_explicit {
            self.mode = parent.mode;
        }
        parent
            .mappings
            .overlay(std::mem::take(&mut self.mappings), work, limits)?;
        self.mappings = parent.mappings;
        parent
            .notdef
            .overlay(std::mem::take(&mut self.notdef), work, limits)?;
        self.notdef = parent.notdef;
        for (code, text) in parent.unicode {
            if !self.unicode.contains_key(&code) {
                if self.unicode.len() >= limits.value_nodes {
                    return Err(Error::limit("inherited Unicode mapping limit exceeded"));
                }
                charge(&mut self.unicode_bytes, text.len(), limits.decoded_bytes)?;
                self.unicode.insert(code, text);
            }
        }
        Ok(())
    }

    pub(crate) fn validate(&self, limits: Limits, work: &mut usize) -> Result<()> {
        for (i, a) in self.spaces.iter().enumerate() {
            for b in &self.spaces[..i] {
                charge(work, 1, limits.operations)?;
                let shared = usize::from(a.first.len.min(b.first.len));
                let overlaps = (0..shared).all(|index| {
                    let shift_a = 8 * (usize::from(a.first.len) - 1 - index);
                    let shift_b = 8 * (usize::from(b.first.len) - 1 - index);
                    let lo_a = (a.first.value >> shift_a) & 255;
                    let hi_a = (a.last.value >> shift_a) & 255;
                    let lo_b = (b.first.value >> shift_b) & 255;
                    let hi_b = (b.last.value >> shift_b) & 255;
                    lo_a <= hi_b && lo_b <= hi_a
                });
                if overlaps {
                    return Err(Error::syntax("overlapping or ambiguous CMap codespaces"));
                }
            }
        }
        if self.spaces.is_empty() {
            return Err(Error::syntax("CMap has no codespace"));
        }
        Ok(())
    }

    pub(crate) fn simple_codes(&self) -> bool {
        self.spaces.iter().all(|range| range.first.len == 1)
    }

    pub(crate) fn next(
        &self,
        input: &[u8],
        work: &mut usize,
        limits: Limits,
    ) -> Result<(Code, bool)> {
        let mut code = 0;
        for (i, byte) in input.iter().take(4).enumerate() {
            code = (code << 8) | u32::from(*byte);
            let len = u8::try_from(i + 1).map_err(|_| Error::syntax("invalid code length"))?;
            charge(work, self.spaces.len(), limits.operations)?;
            if self
                .spaces
                .iter()
                .any(|r| r.first.len == len && r.prefix(&input[..=i]))
            {
                return Ok((Code { value: code, len }, true));
            }
        }
        // Undefined input consumes the shortest range with the longest matching prefix.
        let mut best = (0, usize::MAX);
        for range in &self.spaces {
            charge(work, 4, limits.operations)?;
            let len = usize::from(range.first.len);
            let matched = (1..=len.min(input.len()))
                .take_while(|n| range.prefix(&input[..*n]))
                .count();
            if matched > best.0 || (matched == best.0 && len < best.1) {
                best = (matched, len);
            }
        }
        if best.1 == usize::MAX {
            return Err(Error::syntax("CMap has no codespace"));
        }
        let len = input.len().min(best.1);
        Ok((
            Code {
                value: input
                    .iter()
                    .take(len)
                    .fold(0, |v, b| (v << 8) | u32::from(*b)),
                len: u8::try_from(len).map_err(|_| Error::syntax("invalid code length"))?,
            },
            false,
        ))
    }

    pub(crate) fn cid(&self, code: Code, work: &mut usize, limits: Limits) -> Result<Option<u32>> {
        self.mappings.get(code, work, limits)
    }
    pub(crate) fn notdef(&self, code: Code, work: &mut usize, limits: Limits) -> Result<u32> {
        Ok(self.notdef.get(code, work, limits)?.unwrap_or(0))
    }
    pub(crate) fn text(&self, code: Code) -> Option<&str> {
        self.unicode.get(&code).map(String::as_str)
    }
}

pub(crate) fn charge(work: &mut usize, amount: usize, limit: usize) -> Result<()> {
    *work = work
        .checked_add(amount)
        .filter(|n| *n <= limit)
        .ok_or_else(|| Error::limit("text work budget exceeded"))?;
    Ok(())
}

fn self_definition(map: &mut CMap, operands: &[Value]) -> Result<()> {
    if operands.len() >= 2
        && operands[operands.len() - 2]
            .as_name()
            .is_some_and(|name| name == b"WMode")
    {
        map.mode = mode(operands.last().and_then(Value::as_integer))?;
        map.mode_explicit = true;
    }
    Ok(())
}

fn mode(value: Option<i64>) -> Result<WritingMode> {
    match value {
        Some(0) => Ok(WritingMode::Horizontal),
        Some(1) => Ok(WritingMode::Vertical),
        _ => Err(Error::syntax("invalid CMap WMode")),
    }
}

fn utf16(bytes: &[u8]) -> Result<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err(Error::syntax("odd UTF-16BE destination length"));
    }
    char::decode_utf16(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes([b[0], b[1]])),
    )
    .collect::<std::result::Result<String, _>>()
    .map_err(|_| Error::syntax("invalid UTF-16BE destination"))
}

pub(crate) fn load(document: &Document, value: &Value, unicode: bool) -> Result<CMap> {
    load_inner(
        document,
        value,
        unicode,
        &mut Vec::new(),
        &mut LoadBudget::default(),
    )
}

#[derive(Default)]
struct LoadBudget {
    bytes: usize,
    entries: usize,
    text: usize,
    work: usize,
}

fn load_inner(
    document: &Document,
    value: &Value,
    unicode: bool,
    path: &mut Vec<ObjectId>,
    budget: &mut LoadBudget,
) -> Result<CMap> {
    if let Some(name) = document.resolve(value)?.as_name() {
        if unicode {
            return Err(Error::unsupported(
                features::PREDEFINED_CMAP,
                "predefined ToUnicode CMap data is not bundled",
            ));
        }
        return CMap::identity(name.as_bytes());
    }
    let id = value
        .as_reference()
        .ok_or_else(|| Error::syntax("CMap must be a name or stream reference"))?;
    if path.contains(&id) {
        return Err(Error::cycle("UseCMap cycle").in_object(id));
    }
    let limits = document.limits();
    if path.len() >= limits.depth {
        return Err(Error::limit("UseCMap depth exceeded").in_object(id));
    }
    path.push(id);
    let object = document.object(id)?;
    let dict = object
        .dictionary()
        .ok_or_else(|| Error::syntax("CMap is not a stream"))?;
    let data = document.stream_data(object)?;
    charge(&mut budget.bytes, data.len(), limits.decoded_bytes)?;
    let parse_limits = Limits {
        value_nodes: limits.value_nodes.saturating_sub(budget.entries),
        decoded_bytes: limits.decoded_bytes.saturating_sub(budget.text),
        operations: limits.operations.saturating_sub(budget.work),
        ..limits
    };
    let mut map = CMap::parse(&data, Origin::Stream(id), parse_limits, unicode)
        .map_err(|e| e.in_object(id))?;
    charge(&mut budget.work, map.parse_work, limits.operations).map_err(|e| e.in_object(id))?;
    let entries = map
        .spaces
        .len()
        .saturating_add(map.mappings.len())
        .saturating_add(map.notdef.len())
        .saturating_add(map.unicode.len());
    charge(&mut budget.entries, entries, limits.value_nodes).map_err(|e| e.in_object(id))?;
    charge(&mut budget.text, map.unicode_bytes, limits.decoded_bytes)
        .map_err(|e| e.in_object(id))?;
    if let Some(value) = dict.get(b"WMode") {
        let declared = mode(document.resolve(value)?.as_integer())?;
        if map.mode_explicit && map.mode != declared {
            return Err(Error::syntax("CMap WMode disagrees with stream dictionary").in_object(id));
        }
        map.mode = declared;
        map.mode_explicit = true;
    }
    if let Some(parent) = dict.get(b"UseCMap") {
        if let Some(name) = &map.use_name {
            let parent_name = if let Some(name) = document.resolve(parent)?.as_name() {
                Some(name)
            } else {
                document.resolve_dictionary(parent)?.get_name(b"CMapName")
            };
            if parent_name != Some(name) {
                return Err(Error::syntax("usecmap disagrees with UseCMap").in_object(id));
            }
        }
        let parent = load_inner(document, parent, unicode, path, budget)?;
        map.inherit(parent, limits, &mut budget.work)?;
    } else if map.use_name.is_some() {
        return Err(Error::syntax("usecmap missing UseCMap dictionary entry").in_object(id));
    }
    map.validate(limits, &mut budget.work)
        .map_err(|e| e.in_object(id))?;
    path.pop();
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
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
                CMap::parse(data, Origin::Buffer, Limits::default(), true).is_err(),
                "{data:?}"
            );
        }
    }
    #[test]
    fn huge_unicode_ranges_and_budgeted_work_fail_before_expansion() {
        let limits = Limits {
            value_nodes: 10,
            ..Limits::default()
        };
        assert!(
            CMap::parse(
                b"1 beginbfrange <00000000> <ffffffff> <0041> endbfrange",
                Origin::Buffer,
                limits,
                true
            )
            .is_err()
        );
        let limits = Limits {
            decoded_bytes: 1,
            ..Limits::default()
        };
        assert!(
            CMap::parse(
                b"1 beginbfchar <00> <D83DDE00> endbfchar",
                Origin::Buffer,
                limits,
                true
            )
            .is_err()
        );
    }
    #[test]
    fn identity_lengths_and_word_spacing() -> Result<()> {
        let map = CMap::identity(b"Identity-H")?;
        let (code, valid) = map.next(b"\0 ", &mut 0, Limits::default())?;
        assert!(valid);
        assert_eq!(code, Code { value: 32, len: 2 });
        let (code, valid) = map.next(b"\0", &mut 0, Limits::default())?;
        assert!(!valid);
        assert_eq!(code.len, 1);
        Ok(())
    }
}
