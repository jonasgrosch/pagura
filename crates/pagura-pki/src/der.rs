//! Bounded ASN.1 encodings, X.690 §§8–11.
//!
//! Parsing validates the complete tree iteratively. SET versus SET OF and
//! IMPLICIT types require a schema; use [`Element::set_of`] and the typed
//! views for those checks. Unknown universal types fail explicitly.

use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Position, Result};
use std::ops::Range;

use crate::{features, tags, types};

/// Identifier class (X.690 §8.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// ASN.1 universal type.
    Universal,
    /// Application-defined type.
    Application,
    /// Schema-defined context tag.
    Context,
    /// Private type.
    Private,
}

/// An ASN.1 identifier, including its primitive/constructed bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tag {
    /// Identifier class.
    pub class: Class,
    /// Whether content consists of nested encodings.
    pub constructed: bool,
    /// Tag number; values exceeding u32 are a resource-limit error.
    pub number: u32,
}

impl Tag {
    /// Creates a universal identifier.
    pub const fn universal(number: u32, constructed: bool) -> Self {
        Self {
            class: Class::Universal,
            constructed,
            number,
        }
    }

    /// Creates a context-specific identifier.
    pub const fn context(number: u32, constructed: bool) -> Self {
        Self {
            class: Class::Context,
            constructed,
            number,
        }
    }
}

/// Universal SEQUENCE.
pub const SEQUENCE: Tag = Tag::universal(16, true);
/// Universal SET or SET OF; its schema decides which ordering applies.
pub const SET: Tag = Tag::universal(17, true);

#[derive(Debug)]
struct Node {
    tag: Tag,
    encoded: Range<usize>,
    content: Range<usize>,
    after: usize,
    indefinite: bool,
}

/// A validated encoding and its flat, bounded element index.
///
/// `file_bytes` bounds input; `depth` bounds nesting; `value_nodes` bounds
/// elements; `document_bytes` bounds index storage; `string_bytes` bounds
/// primitive contents. Tree destruction does not recurse.
#[derive(Debug)]
pub struct Document<'a> {
    bytes: &'a [u8],
    nodes: Vec<Node>,
    base: usize,
    limits: Limits,
    ber: bool,
}

impl<'a> Document<'a> {
    /// Parses exactly one DER value with no trailing bytes.
    ///
    /// # Errors
    /// Rejects malformed, noncanonical, unsupported or over-budget input.
    pub fn parse(bytes: &'a [u8], limits: &Limits) -> Result<Self> {
        Self::parse_at(bytes, 0, limits)
    }

    pub(crate) fn parse_at(bytes: &'a [u8], base: usize, limits: &Limits) -> Result<Self> {
        let mut diagnostics = Diagnostics::new(pagura_core::Recovery::Strict);
        Self::read(bytes, base, limits, false, &mut diagnostics)
    }

    /// Parses the BER subset needed by CMS, reporting each DER deviation.
    ///
    /// Only indefinite constructed lengths, nonminimal definite lengths and
    /// constructed OCTET STRINGs are tolerated. Primitive canonicality remains
    /// strict. This is not an unrestricted BER implementation. Signed attributes
    /// and certificates must still be checked as DER by their schema.
    ///
    /// # Errors
    /// Rejects invalid BER, unsupported types, exceeded budgets, or any deviation
    /// when `diagnostics` uses strict recovery.
    pub fn parse_ber(
        bytes: &'a [u8],
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        Self::read(bytes, 0, limits, true, diagnostics)
    }

    fn read(
        bytes: &'a [u8],
        base: usize,
        limits: &Limits,
        ber: bool,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self> {
        if bytes.len() > limits.file_bytes || base.checked_add(bytes.len()).is_none() {
            return Err(Error::limit("ASN.1 input byte budget exceeded").at(Position::Buffer(base)));
        }
        let mut document = Self {
            bytes,
            nodes: Vec::new(),
            base,
            limits: *limits,
            ber,
        };
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let mut pos = 0usize;
        loop {
            if let Some(&(index, boundary)) = stack.last() {
                let node = &document.nodes[index];
                let eoc = node.indefinite && bytes.get(pos..pos.saturating_add(2)) == Some(&[0, 0]);
                if eoc || (!node.indefinite && pos == boundary) {
                    if eoc {
                        document.nodes[index].content.end = pos;
                        pos += 2;
                        if pos > boundary {
                            return Err(document.error(pos - 2, "end-of-contents exceeds parent"));
                        }
                        document.nodes[index].encoded.end = pos;
                    }
                    document.nodes[index].after = document.nodes.len();
                    stack.pop();
                    continue;
                }
                if pos >= boundary {
                    return Err(document.error(pos, "missing end-of-contents"));
                }
            } else if !document.nodes.is_empty() {
                if pos != bytes.len() {
                    return Err(document.error(pos, "trailing ASN.1 data"));
                }
                break;
            }
            if stack.len() >= limits.depth {
                return Err(document.limit(pos, "ASN.1 depth budget exceeded"));
            }
            let boundary = stack.last().map_or(bytes.len(), |&(_, end)| end);
            let index = document.nodes.len();
            let max_nodes = limits
                .value_nodes
                .min(limits.document_bytes / size_of::<Node>());
            if index >= max_nodes {
                return Err(document.limit(pos, "ASN.1 element budget exceeded"));
            }
            // Geometric growth is capped before admission, including spare capacity.
            if document.nodes.len() == document.nodes.capacity() {
                let additional = (max_nodes - index).min(index.max(8));
                document
                    .nodes
                    .try_reserve_exact(additional)
                    .map_err(|_| document.limit(pos, "ASN.1 index allocation failed"))?;
            }
            let start = pos;
            let tag = read_tag(bytes, &mut pos, boundary, base)?;
            if let Some(&(parent, _)) = stack.last() {
                let parent_tag = document.nodes[parent].tag;
                if parent_tag == Tag::universal(4, true)
                    && (tag.class != Class::Universal || tag.number != 4)
                {
                    return Err(document.error(start, "non-OCTET STRING component"));
                }
            }
            let length = read_length(bytes, &mut pos, boundary, base, ber, diagnostics)?;
            let indefinite = length.is_none();
            if indefinite && !tag.constructed {
                return Err(document.error(start, "primitive indefinite length"));
            }
            let end = match length {
                Some(length) => pos
                    .checked_add(length)
                    .filter(|&end| end <= boundary)
                    .ok_or_else(|| document.error(pos, "ASN.1 content exceeds parent"))?,
                None => boundary,
            };
            if !tag.constructed && end - pos > limits.string_bytes {
                return Err(document.limit(pos, "ASN.1 primitive byte budget exceeded"));
            }
            let node = Node {
                tag,
                encoded: start..end,
                content: pos..end,
                after: index + 1,
                indefinite,
            };
            document.nodes.push(node);
            if tag.constructed {
                validate_constructed(tag, start + base, ber, diagnostics)?;
                stack.push((index, end));
            } else {
                types::validate_primitive(tag, &bytes[pos..end], pos + base)?;
                pos = end;
            }
        }
        Ok(document)
    }

    /// The sole top-level element.
    pub fn root(&self) -> Element<'_> {
        Element {
            document: self,
            index: 0,
        }
    }

    fn error(&self, pos: usize, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.base + pos))
    }

    fn limit(&self, pos: usize, message: &'static str) -> Error {
        Error::limit(message).at(Position::Buffer(self.base + pos))
    }
}

/// A borrowed element in a validated [`Document`].
#[derive(Debug, Clone, Copy)]
pub struct Element<'a> {
    document: &'a Document<'a>,
    index: usize,
}

impl<'a> Element<'a> {
    /// The tag, including its constructed bit.
    pub fn tag(self) -> Tag {
        self.node().tag
    }
    /// Exact original tag, length and content bytes, including BER EOC if present.
    pub fn encoded(self) -> &'a [u8] {
        &self.document.bytes[self.node().encoded.clone()]
    }
    /// Content bytes, excluding the tag, length and any EOC marker.
    pub fn content(self) -> &'a [u8] {
        &self.document.bytes[self.node().content.clone()]
    }
    /// Exact range in the input buffer passed to the root parser.
    pub fn range(self) -> Range<usize> {
        self.node().encoded.start + self.document.base..self.node().encoded.end + self.document.base
    }
    /// Position of the first content byte in that buffer.
    pub fn content_offset(self) -> usize {
        self.node().content.start + self.document.base
    }
    /// The limits under which this tree was admitted.
    pub fn limits(self) -> &'a Limits {
        &self.document.limits
    }
    /// Whether this tree was read using the explicit BER entry point.
    pub fn is_ber(self) -> bool {
        self.document.ber
    }
    /// Whether this value uses an indefinite BER length.
    pub fn is_indefinite(self) -> bool {
        self.node().indefinite
    }
    /// Direct children, with nested descendants skipped.
    pub fn children(self) -> Children<'a> {
        Children {
            element: self,
            next: self.index + 1,
            end: self.node().after,
        }
    }
    /// Visits this element and its descendants in encoded preorder. This walks
    /// the flat index in linear time and allocates no traversal stack.
    pub fn walk(self) -> Walk<'a> {
        Walk {
            element: self,
            next: self.index,
            end: self.node().after,
        }
    }
    /// Checks the expected identifier.
    ///
    /// # Errors
    /// Returns a syntax error at the identifier on mismatch.
    pub fn expect(self, tag: Tag) -> Result<Self> {
        if self.tag() == tag {
            Ok(self)
        } else {
            Err(self.error("unexpected ASN.1 tag"))
        }
    }
    /// Validates ascending lexicographic encodings for a schema's SET OF.
    /// X.690 §11.6; duplicates, when legal for the type, are retained.
    ///
    /// # Errors
    /// Rejects an unordered set or a primitive value.
    pub fn set_of(self) -> Result<Children<'a>> {
        if !self.tag().constructed {
            return Err(self.error("SET OF must be constructed"));
        }
        let mut previous: Option<&[u8]> = None;
        for child in self.children() {
            if previous.is_some_and(|bytes| bytes > child.encoded()) {
                return Err(child.error("noncanonical SET OF order"));
            }
            previous = Some(child.encoded());
        }
        Ok(self.children())
    }
    /// A syntax error located at this identifier.
    pub fn error(self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.range().start))
    }
    fn node(self) -> &'a Node {
        &self.document.nodes[self.index]
    }
}

/// Iterator over direct children of an element.
#[derive(Debug, Clone)]
pub struct Children<'a> {
    element: Element<'a>,
    next: usize,
    end: usize,
}

/// Iterator over an element and all its descendants in encoded order.
#[derive(Debug, Clone)]
pub struct Walk<'a> {
    element: Element<'a>,
    next: usize,
    end: usize,
}

impl<'a> Iterator for Walk<'a> {
    type Item = Element<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let element = Element {
            document: self.element.document,
            index: self.next,
        };
        self.next += 1;
        Some(element)
    }
}

impl<'a> Iterator for Children<'a> {
    type Item = Element<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let child = Element {
            document: self.element.document,
            index: self.next,
        };
        self.next = child.node().after;
        Some(child)
    }
}

/// A schema cursor over an already bounded constructed value.
#[derive(Debug)]
pub(crate) struct Fields<'a> {
    parent: Element<'a>,
    children: std::iter::Peekable<Children<'a>>,
}

impl<'a> Fields<'a> {
    pub(crate) fn sequence(parent: Element<'a>) -> Result<Self> {
        parent.expect(SEQUENCE)?;
        Ok(Self::new(parent))
    }
    pub(crate) fn new(parent: Element<'a>) -> Self {
        Self {
            parent,
            children: parent.children().peekable(),
        }
    }
    pub(crate) fn take(&mut self) -> Result<Element<'a>> {
        self.children
            .next()
            .ok_or_else(|| self.parent.error("missing ASN.1 field"))
    }
    pub(crate) fn optional(&mut self, tag: Tag) -> Option<Element<'a>> {
        if self
            .children
            .peek()
            .is_some_and(|element| element.tag() == tag)
        {
            self.children.next()
        } else {
            None
        }
    }
    pub(crate) fn rest(&mut self) -> Option<Element<'a>> {
        self.children.next()
    }
    pub(crate) fn finish(mut self) -> Result<()> {
        if let Some(extra) = self.children.next() {
            Err(extra.error("unexpected ASN.1 field"))
        } else {
            Ok(())
        }
    }
}

pub(crate) fn single(element: Element<'_>) -> Result<Element<'_>> {
    let mut fields = Fields::new(element);
    let value = fields.take()?;
    fields.finish()?;
    Ok(value)
}

fn byte(bytes: &[u8], pos: &mut usize, end: usize, base: usize) -> Result<u8> {
    let value = bytes
        .get(*pos)
        .filter(|_| *pos < end)
        .copied()
        .ok_or_else(|| Error::syntax("truncated ASN.1 header").at(Position::Buffer(base + *pos)))?;
    *pos += 1;
    Ok(value)
}

fn read_tag(bytes: &[u8], pos: &mut usize, end: usize, base: usize) -> Result<Tag> {
    let start = *pos;
    let first = byte(bytes, pos, end, base)?;
    let class = match first >> 6 {
        0 => Class::Universal,
        1 => Class::Application,
        2 => Class::Context,
        _ => Class::Private,
    };
    let mut number = u32::from(first & 31);
    if number == 31 {
        number = 0;
        loop {
            let value = byte(bytes, pos, end, base)?;
            if number == 0 && value.trailing_zeros() >= 7 {
                return Err(
                    Error::syntax("nonminimal ASN.1 tag").at(Position::Buffer(base + start))
                );
            }
            number = number
                .checked_mul(128)
                .and_then(|n| n.checked_add(u32::from(value & 127)))
                .ok_or_else(|| {
                    Error::limit("ASN.1 tag number exceeds u32").at(Position::Buffer(base + start))
                })?;
            if value & 128 == 0 {
                break;
            }
        }
        if number < 31 {
            return Err(Error::syntax("nonminimal ASN.1 tag").at(Position::Buffer(base + start)));
        }
    }
    Ok(Tag {
        class,
        constructed: first & 32 != 0,
        number,
    })
}

fn read_length(
    bytes: &[u8],
    pos: &mut usize,
    end: usize,
    base: usize,
    ber: bool,
    diagnostics: &mut Diagnostics,
) -> Result<Option<usize>> {
    let start = *pos;
    let first = byte(bytes, pos, end, base)?;
    if first < 128 {
        return Ok(Some(usize::from(first)));
    }
    let count = usize::from(first & 127);
    if count == 0 {
        deviation(
            ber,
            diagnostics,
            tags::BER_INDEFINITE,
            base + start,
            "indefinite ASN.1 length is not DER",
        )?;
        return Ok(None);
    }
    if first == 255 {
        return Err(Error::syntax("reserved ASN.1 length").at(Position::Buffer(base + start)));
    }
    if count > size_of::<usize>() {
        return Err(
            Error::limit("ASN.1 length exceeds address space").at(Position::Buffer(base + start))
        );
    }
    let mut length = 0usize;
    let mut leading_zero = false;
    for index in 0..count {
        let value = byte(bytes, pos, end, base)?;
        leading_zero |= index == 0 && value == 0;
        length = length
            .checked_mul(256)
            .and_then(|n| n.checked_add(usize::from(value)))
            .ok_or_else(|| {
                Error::limit("ASN.1 length overflow").at(Position::Buffer(base + start))
            })?;
    }
    if leading_zero || length < 128 {
        deviation(
            ber,
            diagnostics,
            tags::BER_LENGTH,
            base + start,
            "nonminimal ASN.1 length",
        )?;
    }
    Ok(Some(length))
}

fn deviation(
    ber: bool,
    diagnostics: &mut Diagnostics,
    repair: pagura_core::Repair,
    pos: usize,
    message: &'static str,
) -> Result<()> {
    let diagnostic = Diagnostic::new(repair, message).at(Position::Buffer(pos));
    if ber {
        diagnostics.repair(diagnostic)
    } else {
        Err(diagnostic.to_error())
    }
}

fn validate_constructed(
    tag: Tag,
    pos: usize,
    ber: bool,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if tag.class != Class::Universal {
        return Ok(());
    }
    match tag.number {
        16 | 17 => Ok(()),
        4 => deviation(
            ber,
            diagnostics,
            tags::BER_OCTETS,
            pos,
            "constructed OCTET STRING is not DER",
        ),
        _ => Err(Error::unsupported(
            features::CONSTRUCTED_TYPE,
            "unsupported constructed universal type",
        )
        .at(Position::Buffer(pos))),
    }
}
