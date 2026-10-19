//! Bounded ASN.1 encodings (X.690 8-11).
//!
//! Parsing validates the complete tree iteratively. SET versus SET OF and
//! IMPLICIT types require a schema; use [`Element::set_of`], [`Fields`] and
//! the typed views for those checks. Unknown universal types fail explicitly.

mod reader;
mod tag;

pub(crate) use tag::universal;
pub use tag::{
    BIT_STRING, BMP_STRING, BOOLEAN, Class, ENUMERATED, Form, GENERAL_STRING, GENERALIZED_TIME,
    GRAPHIC_STRING, IA5_STRING, INTEGER, NULL, NUMERIC_STRING, OBJECT_IDENTIFIER, OCTET_STRING,
    PRINTABLE_STRING, RELATIVE_OID, SEQUENCE, SET, TELETEX_STRING, Tag, UNIVERSAL_STRING, UTC_TIME,
    UTF8_STRING, VIDEOTEX_STRING, VISIBLE_STRING,
};

use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Position, Recovery, Result};
use reader::{Encoding, Extent};
use std::iter::Peekable;
use std::ops::Range;

#[derive(Debug)]
struct Node {
    tag: Tag,
    encoded: Range<usize>,
    content: Range<usize>,
    /// Index of the first node after this element's descendants.
    after: usize,
    indefinite: bool,
}

/// A validated encoding and its flat, bounded element index.
///
/// `Limits::asn1_depth` bounds nesting, `asn1_elements` the element count and
/// `asn1_bytes` the index and traversal stack. Tree destruction does not
/// recurse.
#[derive(Debug)]
pub struct Document<'a> {
    bytes: &'a [u8],
    nodes: Vec<Node>,
    base: usize,
    limits: Limits,
    encoding: Encoding,
}

impl<'a> Document<'a> {
    /// Parses exactly one DER value with no trailing bytes.
    ///
    /// # Errors
    /// Rejects malformed, noncanonical, unsupported or over-budget input.
    pub fn parse(bytes: &'a [u8], limits: &Limits) -> Result<Self> {
        Self::parse_at(bytes, 0, limits)
    }

    /// Parses DER that starts at offset `base` of the caller's buffer, so
    /// positions and ranges refer to that buffer.
    pub(crate) fn parse_at(bytes: &'a [u8], base: usize, limits: &Limits) -> Result<Self> {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        reader::read(
            bytes,
            base,
            limits,
            Encoding::Der,
            Extent::Whole,
            &mut diagnostics,
        )
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
        reader::read(bytes, 0, limits, Encoding::Ber, Extent::Whole, diagnostics)
    }

    /// Parses the first DER value and returns it with its length. The bytes
    /// after it are the caller's, such as the zero padding of a PDF signature's
    /// `/Contents` (12.8.3.3.1); zero octets inside the value are kept.
    ///
    /// # Errors
    /// Rejects malformed, noncanonical, unsupported or over-budget input.
    pub fn parse_prefix(bytes: &'a [u8], limits: &Limits) -> Result<(Self, usize)> {
        let mut diagnostics = Diagnostics::new(Recovery::Strict);
        let document = reader::read(
            bytes,
            0,
            limits,
            Encoding::Der,
            Extent::Prefix,
            &mut diagnostics,
        )?;
        let consumed = document.bytes.len();
        Ok((document, consumed))
    }

    /// The BER counterpart of [`Self::parse_prefix`], reporting deviations as
    /// [`Self::parse_ber`] does.
    ///
    /// # Errors
    /// As [`Self::parse_ber`]; the bytes after the value are not read as ASN.1.
    pub fn parse_ber_prefix(
        bytes: &'a [u8],
        limits: &Limits,
        diagnostics: &mut Diagnostics,
    ) -> Result<(Self, usize)> {
        let document = reader::read(bytes, 0, limits, Encoding::Ber, Extent::Prefix, diagnostics)?;
        let consumed = document.bytes.len();
        Ok((document, consumed))
    }

    /// The sole top-level element.
    pub fn root(&self) -> Element<'_> {
        Element {
            document: self,
            index: 0,
        }
    }
}

/// A borrowed element in a validated [`Document`].
#[derive(Debug, Clone, Copy)]
pub struct Element<'a> {
    document: &'a Document<'a>,
    index: usize,
}

impl<'a> Element<'a> {
    /// The identifier, including its form.
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
        let encoded = &self.node().encoded;
        encoded.start + self.document.base..encoded.end + self.document.base
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
        self.document.encoding == Encoding::Ber
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
    /// X.690 11.6; duplicates, when legal for the type, are retained.
    ///
    /// # Errors
    /// Rejects an unordered set or a primitive value.
    pub fn set_of(self) -> Result<Children<'a>> {
        if !self.tag().is_constructed() {
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
    /// Iterates a schema's SET OF in source order. In a BER tree a set out of
    /// DER order is reported as [`repairs::BER_SET_ORDER`](crate::repairs::BER_SET_ORDER)
    /// (X.690 11.6); in a DER tree it is rejected as by [`Self::set_of`].
    /// Signed attributes always use the strict [`Self::set_of`].
    ///
    /// # Errors
    /// Rejects a primitive value, DER misordering, and BER misordering when
    /// `diagnostics` is strict.
    pub fn set_of_with_diagnostics(self, diagnostics: &mut Diagnostics) -> Result<Children<'a>> {
        match self.set_of() {
            Err(error) if self.is_ber() && self.tag().is_constructed() => {
                diagnostics.repair(
                    Diagnostic::new(crate::repairs::BER_SET_ORDER, error.message)
                        .at(error.position),
                )?;
                Ok(self.children())
            }
            result => result,
        }
    }
    /// A syntax error located at this identifier.
    pub fn error(self, message: &'static str) -> Error {
        Error::syntax(message).at(Position::Buffer(self.range().start))
    }
    /// Checks a component of a BER tree again as DER, without normalizing any
    /// byte: certificates, CRLs, TSTInfo and signature envelopes are DER
    /// even inside a BER container. A DER tree needs no second pass.
    pub(crate) fn require_der(self) -> Result<()> {
        if self.is_ber() {
            Document::parse_at(self.encoded(), self.range().start, self.limits())?;
        }
        Ok(())
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

/// A schema cursor over the fields of an already admitted constructed value,
/// read in schema order: required fields with [`Fields::take`], OPTIONAL and
/// DEFAULT ones by tag with [`Fields::optional`], and no field left over.
#[derive(Debug)]
pub struct Fields<'a> {
    parent: Element<'a>,
    children: Peekable<Children<'a>>,
}

impl<'a> Fields<'a> {
    /// The fields of a SEQUENCE.
    ///
    /// # Errors
    /// Rejects any other identifier.
    pub fn sequence(parent: Element<'a>) -> Result<Self> {
        parent.expect(SEQUENCE)?;
        Ok(Self::new(parent))
    }
    /// The fields of any constructed value, for IMPLICIT or EXPLICIT tags.
    pub fn new(parent: Element<'a>) -> Self {
        Self {
            parent,
            children: parent.children().peekable(),
        }
    }
    /// The next required field.
    ///
    /// # Errors
    /// Returns a syntax error at the parent when no field remains.
    pub fn take(&mut self) -> Result<Element<'a>> {
        self.children
            .next()
            .ok_or_else(|| self.parent.error("missing ASN.1 field"))
    }
    /// The next field if it carries `tag`.
    pub fn optional(&mut self, tag: Tag) -> Option<Element<'a>> {
        self.children.next_if(|element| element.tag() == tag)
    }
    /// The next field whatever its tag, for a trailing `ANY OPTIONAL`.
    pub fn optional_any(&mut self) -> Option<Element<'a>> {
        self.children.next()
    }
    /// Checks that every field was read.
    ///
    /// # Errors
    /// Returns a syntax error at the first unread field.
    pub fn finish(mut self) -> Result<()> {
        match self.children.next() {
            Some(extra) => Err(extra.error("unexpected ASN.1 field")),
            None => Ok(()),
        }
    }
}

/// The only component of a constructed value, such as an EXPLICIT tag's.
///
/// # Errors
/// Rejects a value with no component or with more than one.
pub fn single(element: Element<'_>) -> Result<Element<'_>> {
    let mut fields = Fields::new(element);
    let value = fields.take()?;
    fields.finish()?;
    Ok(value)
}
