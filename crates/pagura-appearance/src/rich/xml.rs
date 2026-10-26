//! Small bounded XML reader for inert XFA rich text. No DTD, entity expansion,
//! processing instructions other than the XML declaration, or external objects.

use super::style::Style;
use pagura_core::{Error, Limits, Meter, Result};
use std::ops::Range;

pub(super) struct Character {
    pub(super) value: char,
    pub(super) style: usize,
}

pub(super) struct Paragraph {
    pub(super) characters: Range<usize>,
    pub(super) style: usize,
}

pub(crate) struct Text {
    pub(super) styles: Vec<Style>,
    pub(super) characters: Vec<Character>,
    pub(super) paragraphs: Vec<Paragraph>,
}

struct Frame<'a> {
    name: &'a str,
    style: usize,
    ignored: bool,
}

struct Reader<'a, 'm> {
    work: &'m mut Meter,
    text: Text,
    stack: Vec<Frame<'a>>,
    storage: &'m mut Meter,
    nodes: Meter,
    depth: usize,
    start: usize,
    paragraph: Option<usize>,
    pending_space: Option<usize>,
    root_seen: bool,
}

impl Text {
    /// Plain V uses the parsed characters, with LF between rich paragraphs.
    /// Formatting whitespace and entities follow the same reader as the glyphs.
    pub(crate) fn value(
        &self,
        limits: &Limits,
        storage: &mut Meter,
        work: &mut Meter,
    ) -> Result<String> {
        work.charge(self.characters.len().saturating_add(self.paragraphs.len()))?;
        let bytes = self
            .characters
            .iter()
            .fold(0usize, |bytes, character| {
                bytes.saturating_add(character.value.len_utf8())
            })
            .saturating_add(self.paragraphs.len().saturating_sub(1));
        if bytes > limits.string_bytes {
            return Err(Error::limit("rich field plain value exceeds string budget"));
        }
        storage.charge(bytes)?;
        let mut value = String::with_capacity(bytes);
        for (index, paragraph) in self.paragraphs.iter().enumerate() {
            if index != 0 {
                value.push('\n');
            }
            value.extend(
                self.characters[paragraph.characters.clone()]
                    .iter()
                    .map(|c| c.value),
            );
        }
        Ok(value)
    }

    /// Password previews never emit the original characters, including spaces.
    pub(crate) fn mask(&mut self, work: &mut Meter) -> Result<()> {
        work.charge(self.characters.len())?;
        for character in &mut self.characters {
            if character.value != '\n' {
                character.value = '*';
            }
        }
        Ok(())
    }

    pub(super) fn parse(
        source: &str,
        base: Style,
        limits: &Limits,
        work: &mut Meter,
        storage: &mut Meter,
    ) -> Result<Self> {
        let mut reader = Reader::new(base, limits, work, storage)?;
        let mut rest = source.trim_start_matches('\u{feff}');
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix("<!--") {
                let (comment, after) = tail
                    .split_once("-->")
                    .ok_or(Error::syntax("unterminated rich text comment"))?;
                if comment.contains("--") {
                    return Err(Error::syntax("invalid rich text comment"));
                }
                rest = after;
            } else if let Some(tail) = rest.strip_prefix("<![CDATA[") {
                let (text, after) = tail
                    .split_once("]]>")
                    .ok_or(Error::syntax("unterminated rich text CDATA"))?;
                reader.content(text, Content::Literal)?;
                rest = after;
            } else if let Some(tail) = rest.strip_prefix("<?xml ") {
                if reader.root_seen {
                    return Err(Error::syntax("misplaced rich text XML declaration"));
                }
                let (_, after) = tail
                    .split_once("?>")
                    .ok_or(Error::syntax("unterminated rich text declaration"))?;
                rest = after;
            } else if rest.starts_with("<!") || rest.starts_with("<?") {
                return Err(super::unsupported(
                    "rich text declarations and external entities are unavailable",
                ));
            } else if let Some(tail) = rest.strip_prefix("</") {
                let (name, tail) = name(tail)?;
                rest = tail
                    .trim_start()
                    .strip_prefix('>')
                    .ok_or(Error::syntax("invalid rich text closing tag"))?;
                let frame = reader
                    .stack
                    .pop()
                    .ok_or(Error::syntax("unmatched rich text closing tag"))?;
                if frame.name != name {
                    return Err(Error::syntax("mismatched rich text closing tag"));
                }
                if !frame.ignored && name == "p" {
                    reader.end_paragraph(frame.style)?;
                }
            } else if let Some(tail) = rest.strip_prefix('<') {
                rest = reader.open(tail)?;
            } else {
                let (text, after) = rest
                    .find('<')
                    .map_or((rest, ""), |index| rest.split_at(index));
                reader.content(text, Content::Escaped)?;
                rest = after;
            }
        }
        if !reader.root_seen || !reader.stack.is_empty() {
            return Err(Error::syntax("incomplete rich text document"));
        }
        if reader.text.characters.len() > reader.start {
            reader.end_paragraph(reader.paragraph.unwrap_or(0))?;
        }
        Ok(reader.text)
    }

    pub(super) fn plain(
        source: &str,
        base: Style,
        limits: &Limits,
        work: &mut Meter,
        storage: &mut Meter,
    ) -> Result<Self> {
        let mut reader = Reader::new(base, limits, work, storage)?;
        reader.storage.reserve_capacity(&mut reader.stack, 1)?;
        reader.stack.push(Frame {
            name: "body",
            style: 0,
            ignored: false,
        });
        reader.text.styles[0].preserve = true;
        reader.content(source, Content::Literal)?;
        if !reader.text.characters.is_empty() {
            reader.end_paragraph(0)?;
        }
        Ok(reader.text)
    }
}

#[derive(Clone, Copy)]
enum End {
    Empty,
    Open,
}

#[derive(Clone, Copy)]
enum Content {
    Literal,
    Escaped,
}

impl<'a, 'm> Reader<'a, 'm> {
    fn new(
        base: Style,
        limits: &Limits,
        work: &'m mut Meter,
        storage: &'m mut Meter,
    ) -> Result<Self> {
        storage.charge(base.family.heap_bytes())?;
        let mut styles = Vec::new();
        storage.reserve_capacity(&mut styles, 1)?;
        styles.push(base);
        Ok(Self {
            work,
            text: Text {
                styles,
                characters: Vec::new(),
                paragraphs: Vec::new(),
            },
            stack: Vec::new(),
            storage,
            nodes: Meter::new(limits.value_nodes, "rich text node budget exceeded"),
            depth: limits.depth,
            start: 0,
            paragraph: None,
            pending_space: None,
            root_seen: false,
        })
    }

    fn open(&mut self, source: &'a str) -> Result<&'a str> {
        self.work.charge(1)?;
        self.nodes.charge(1)?;
        if self.stack.len() >= self.depth {
            return Err(Error::limit("rich text nesting budget exceeded"));
        }
        if self.stack.is_empty() && self.root_seen {
            return Err(Error::syntax("multiple rich text roots"));
        }
        let (tag, rest) = name(source)?;
        let parent = self.stack.last().map_or(0, |frame| frame.style);
        let ignored = self.stack.last().is_some_and(|frame| frame.ignored)
            || !matches!(
                tag,
                "html" | "body" | "p" | "span" | "b" | "i" | "sub" | "sup" | "br" | "a"
            );
        if matches!(tag, "ol" | "ul" | "li") {
            return Err(super::unsupported("rich text lists are unavailable"));
        }
        self.work.charge(self.text.styles[parent].family.len())?;
        self.storage
            .charge(self.text.styles[parent].family.heap_bytes())?;
        self.storage.reserve_capacity(&mut self.text.styles, 1)?;
        let mut style = self.text.styles[parent].clone();
        match tag {
            "b" => style.bold = true,
            "i" => style.italic = true,
            "sup" => {
                style.rise += style.size * 0.31;
                style.size *= 0.66;
            }
            "sub" => {
                style.rise -= style.size * 0.15;
                style.size *= 0.66;
            }
            _ => (),
        }
        let (rest, end) = self.attributes(rest, (!ignored).then_some(&mut style))?;
        self.storage.charge(style.family.heap_bytes())?;
        let index = self.text.styles.len();
        self.text.styles.push(style);
        if !ignored {
            if tag == "p" {
                self.work.charge(self.stack.len())?;
                if self.stack.iter().any(|frame| frame.name == "p") {
                    return Err(Error::syntax("nested rich text paragraphs"));
                }
                if self.text.characters.len() > self.start {
                    self.end_paragraph(self.paragraph.unwrap_or(parent))?;
                }
                self.paragraph = Some(index);
            } else if tag == "br" {
                self.push('\n', index)?;
            }
        }
        self.root_seen = true;
        if matches!(end, End::Empty) {
            if !ignored && tag == "p" {
                self.end_paragraph(index)?;
            }
        } else {
            self.storage.reserve_capacity(&mut self.stack, 1)?;
            self.stack.push(Frame {
                name: tag,
                style: index,
                ignored,
            });
        }
        Ok(rest)
    }

    fn attributes(
        &mut self,
        mut rest: &'a str,
        mut style: Option<&mut Style>,
    ) -> Result<(&'a str, End)> {
        let mut attributes = Vec::new();
        let empty;
        loop {
            let trimmed = rest.trim_start();
            if let Some(after) = trimmed.strip_prefix("/>") {
                empty = End::Empty;
                rest = after;
                break;
            }
            if let Some(after) = trimmed.strip_prefix('>') {
                empty = End::Open;
                rest = after;
                break;
            }
            if trimmed.len() == rest.len() {
                return Err(Error::syntax("missing rich text attribute separator"));
            }
            let (attribute, tail) = name(trimmed)?;
            let tail = tail
                .trim_start()
                .strip_prefix('=')
                .ok_or(Error::syntax("missing rich text attribute value"))?
                .trim_start();
            let quote = tail
                .chars()
                .next()
                .filter(|c| matches!(c, '\'' | '"'))
                .ok_or(Error::syntax("unquoted rich text attribute"))?;
            let (value, tail) = tail[1..]
                .split_once(quote)
                .ok_or(Error::syntax("unterminated rich text attribute"))?;
            self.work.charge(1)?;
            self.nodes.charge(1)?;
            self.work.charge(attributes.len())?;
            if attributes.contains(&attribute) {
                return Err(Error::syntax("duplicate rich text attribute"));
            }
            self.storage.reserve_capacity(&mut attributes, 1)?;
            attributes.push(attribute);
            self.storage.charge_product(value.len(), 2)?;
            let value = decode(value)?;
            if let Some(style) = style.as_deref_mut() {
                match attribute {
                    "style" => {
                        style.apply(&value)?;
                    }
                    "xmlns" if value != "http://www.w3.org/1999/xhtml" => {
                        return Err(super::unsupported("foreign rich text namespace"));
                    }
                    "xfa:embed" | "xfa:embedType" | "xfa:embedMode" => {
                        return Err(super::unsupported(
                            "rich text external objects are unavailable",
                        ));
                    }
                    "dir" if value != "ltr" => {
                        return Err(super::unsupported(
                            "right-to-left rich text layout is unavailable",
                        ));
                    }
                    _ => (),
                }
            }
            rest = tail;
        }
        Ok((rest, empty))
    }

    fn content(&mut self, source: &str, mode: Content) -> Result<()> {
        let Some(frame) = self.stack.last() else {
            if !source.trim().is_empty() {
                return Err(Error::syntax("text outside rich text root"));
            }
            return Ok(());
        };
        if frame.ignored {
            return Ok(());
        }
        let style = frame.style;
        let preserve = self.text.styles[style].preserve;
        let mut rest = source;
        while !rest.is_empty() {
            let (character, after) = next(rest, mode)?;
            rest = after;
            if character == '\r' {
                if rest.starts_with('\n') {
                    rest = &rest[1..];
                }
                if preserve {
                    self.push('\n', style)?;
                    continue;
                }
            }
            if !preserve && matches!(character, ' ' | '\t' | '\n' | '\r') {
                if self.text.characters.len() > self.start {
                    self.pending_space = Some(style);
                }
            } else {
                if let Some(style) = self.pending_space.take() {
                    self.push(' ', style)?;
                }
                self.push(character, style)?;
            }
        }
        Ok(())
    }

    fn push(&mut self, value: char, style: usize) -> Result<()> {
        self.work.charge(1)?;
        self.nodes.charge(1)?;
        self.storage
            .reserve_capacity(&mut self.text.characters, 1)?;
        self.text.characters.push(Character { value, style });
        Ok(())
    }

    fn end_paragraph(&mut self, style: usize) -> Result<()> {
        self.storage
            .reserve_capacity(&mut self.text.paragraphs, 1)?;
        self.text.paragraphs.push(Paragraph {
            characters: self.start..self.text.characters.len(),
            style,
        });
        self.start = self.text.characters.len();
        self.paragraph = None;
        self.pending_space = None;
        Ok(())
    }
}

fn name(source: &str) -> Result<(&str, &str)> {
    let end = source
        .find(|c: char| !c.is_alphanumeric() && !matches!(c, '_' | '-' | ':' | '.'))
        .unwrap_or(source.len());
    let (name, tail) = source.split_at(end);
    if !name
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | ':'))
    {
        return Err(Error::syntax("invalid rich text XML name"));
    }
    Ok((name, tail))
}

fn decode(source: &str) -> Result<String> {
    if source.contains('<') {
        return Err(Error::syntax("unescaped rich text attribute character"));
    }
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while !rest.is_empty() {
        let (c, after) = next(rest, Content::Escaped)?;
        output.push(c);
        rest = after;
    }
    Ok(output)
}

fn next(source: &str, mode: Content) -> Result<(char, &str)> {
    let (character, tail) = if matches!(mode, Content::Escaped)
        && let Some(tail) = source.strip_prefix('&')
    {
        let (entity, tail) = tail
            .split_once(';')
            .ok_or(Error::syntax("unterminated rich text entity"))?;
        let character = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let number = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16)
                } else if let Some(decimal) = entity.strip_prefix('#') {
                    decimal.parse::<u32>()
                } else {
                    return Err(Error::syntax("undefined rich text entity"));
                };
                number
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or(Error::syntax("invalid rich text character reference"))?
            }
        };
        (character, tail)
    } else {
        let character = source
            .chars()
            .next()
            .ok_or(Error::syntax("missing rich text character"))?;
        (character, &source[character.len_utf8()..])
    };
    if character < '\u{20}' && !matches!(character, '\t' | '\n' | '\r')
        || matches!(character, '\u{fffe}' | '\u{ffff}')
    {
        return Err(Error::syntax("invalid rich text XML character"));
    }
    Ok((character, tail))
}
