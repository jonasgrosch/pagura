//! The iterative reader behind [`Document`]: identifiers and lengths
//! (X.690 8.1), the DER restrictions (X.690 10) and the reported BER subset.

use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Meter, Position, Repair, Result};

use super::tag::{Class, Form, Tag, universal};
use super::{Document, Node};
use crate::{features, repairs, types};

/// Whether the encoding must fill its input or may be followed by other bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Extent {
    /// Exactly one encoding, with no trailing bytes.
    Whole,
    /// The first encoding; the bytes after it are the caller's.
    Prefix,
}

/// The encoding rules a document is read under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
    /// The Distinguished Encoding Rules (X.690 10–11).
    Der,
    /// DER plus the reported BER subset that CMS containers use.
    Ber,
}

const TRUNCATED: &str = "truncated ASN.1 encoding";
const MEMORY: &str = "ASN.1 index byte budget exceeded";

/// Identifier octet: class bits, the constructed bit and the low tag number
/// (X.690 8.1.2.2-8.1.2.4).
const CLASS_SHIFT: u8 = 6;
const CONSTRUCTED_BIT: u8 = 0x20;
const LOW_NUMBER: u8 = 0x1f;
/// Low tag number announcing subsequent number octets (X.690 8.1.2.4.1).
const HIGH_NUMBER: u8 = 0x1f;
/// Bit 8 of a subsequent identifier octet or of a long-form length's first
/// octet (X.690 8.1.2.4.2, 8.1.3.5).
const CONTINUATION: u8 = 0x80;
const SEVEN_BITS: u8 = 0x7f;
/// Long-form initial length octet reserved for extension (X.690 8.1.3.5 c).
const RESERVED_LENGTH: u8 = 0xff;
/// End-of-contents octets closing an indefinite length (X.690 8.1.5).
const END_OF_CONTENTS: [u8; 2] = [0, 0];
/// Shortest length that DER encodes in long form (X.690 10.1).
const LONG_FORM_MINIMUM: usize = 0x80;

/// A constructed element whose children are being read.
#[derive(Debug)]
struct Frame<'a> {
    node: usize,
    /// The element's content when its length is definite. When it is
    /// indefinite, the rest of the parent's content: the element reads up to
    /// its end-of-contents octets and hands the cursor back to its parent.
    content: Cursor<'a>,
    /// Offset of `content`'s first byte in the document input.
    start: usize,
}

struct Reader<'a> {
    base: usize,
    /// `Limits::signatures` bounds on nesting and on indexed elements.
    max_depth: usize,
    max_elements: usize,
    encoding: Encoding,
    nodes: Vec<Node>,
    frames: Vec<Frame<'a>>,
    memory: Meter,
}

/// Reads one encoding and validates the whole tree without recursion. The
/// document covers only the bytes the encoding occupies.
pub(super) fn read<'a>(
    bytes: &'a [u8],
    base: usize,
    limits: &Limits,
    encoding: Encoding,
    extent: Extent,
    diagnostics: &mut Diagnostics,
) -> Result<Document<'a>> {
    if base.checked_add(bytes.len()).is_none() {
        return Err(Error::limit("ASN.1 input offset overflow").at(Position::Buffer(base)));
    }
    let budgets = limits.signatures;
    let mut reader = Reader {
        base,
        max_depth: u32_to_usize(budgets.asn1_depth),
        max_elements: u32_to_usize(budgets.asn1_elements),
        encoding,
        nodes: Vec::new(),
        frames: Vec::new(),
        memory: Meter::new(u32_to_usize(budgets.asn1_bytes), MEMORY),
    };
    let mut top = Cursor::new(bytes).with_base(base).with_message(TRUNCATED);
    loop {
        let Some(mut frame) = reader.frames.pop() else {
            if !reader.nodes.is_empty() {
                break;
            }
            if let Some(root) = reader.element(&mut top, 0, 0, None, diagnostics)? {
                reader.push_frame(root)?;
            }
            continue;
        };
        if reader.close(&mut frame)? {
            if reader.nodes[frame.node].indefinite {
                // The parent continues after the end-of-contents octets.
                match reader.frames.last_mut() {
                    Some(parent) => parent.content = frame.content,
                    None => top = frame.content,
                }
            }
            continue;
        }
        let depth = reader.frames.len() + 1;
        let parent = reader.nodes[frame.node].tag;
        let child = reader.element(
            &mut frame.content,
            frame.start,
            depth,
            Some(parent),
            diagnostics,
        )?;
        reader.frames.push(frame);
        if let Some(child) = child {
            reader.push_frame(child)?;
        }
    }
    if extent == Extent::Whole && !top.at_end() {
        return Err(top.error("trailing ASN.1 data"));
    }
    Ok(Document {
        bytes: bytes.get(..top.offset()).unwrap_or(bytes),
        nodes: reader.nodes,
        base,
        limits: *limits,
        encoding,
    })
}

impl<'a> Reader<'a> {
    /// Closes `frame` if its content is complete, recording where an
    /// indefinite element ends.
    fn close(&mut self, frame: &mut Frame<'a>) -> Result<bool> {
        let after = self.nodes.len();
        let node = &mut self.nodes[frame.node];
        if !node.indefinite {
            if frame.content.at_end() {
                node.after = after;
                return Ok(true);
            }
            return Ok(false);
        }
        if frame.content.rest().starts_with(&END_OF_CONTENTS) {
            node.content.end = frame.start + frame.content.offset();
            frame.content.skip(END_OF_CONTENTS.len())?;
            node.encoded.end = frame.start + frame.content.offset();
            node.after = after;
            return Ok(true);
        }
        if frame.content.at_end() {
            return Err(frame.content.error("missing end-of-contents"));
        }
        Ok(false)
    }

    /// Reads one element with `depth` open ancestors from `cursor`, whose data
    /// starts at input offset `start`, and returns the frame for its children
    /// when it is constructed.
    fn element(
        &mut self,
        cursor: &mut Cursor<'a>,
        start: usize,
        depth: usize,
        parent: Option<Tag>,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Frame<'a>>> {
        let at = cursor.position();
        if depth >= self.max_depth {
            return Err(Error::limit("ASN.1 depth budget exceeded").at(at));
        }
        if self.nodes.len() >= self.max_elements {
            return Err(Error::limit("ASN.1 element budget exceeded").at(at));
        }
        let begin = start + cursor.offset();
        let tag = read_tag(cursor)?;
        if parent.is_some_and(|parent| parent.is_universal(universal::OCTET_STRING))
            && !tag.is_universal(universal::OCTET_STRING)
        {
            return Err(Error::syntax("non-OCTET STRING component").at(at));
        }
        let length = read_length(cursor, self.encoding, diagnostics)?;
        let content_start = start + cursor.offset();
        let index = self.nodes.len();
        let Some(length) = length else {
            if tag.form == Form::Primitive {
                return Err(Error::syntax("primitive indefinite length").at(at));
            }
            // The extent is known only at the end-of-contents octets.
            self.push_node(Node {
                tag,
                encoded: begin..begin,
                content: content_start..content_start,
                after: index + 1,
                indefinite: true,
            })?;
            validate_constructed(tag, at, self.encoding, diagnostics)?;
            return Ok(Some(Frame {
                node: index,
                content: cursor.clone(),
                start,
            }));
        };
        let content = cursor.take_cursor(length)?;
        let end = content_start + length;
        self.push_node(Node {
            tag,
            encoded: begin..end,
            content: content_start..end,
            after: index + 1,
            indefinite: false,
        })?;
        match tag.form {
            Form::Primitive => {
                types::validate_primitive(tag, content.data(), self.base + content_start)?;
                Ok(None)
            }
            Form::Constructed => {
                validate_constructed(tag, at, self.encoding, diagnostics)?;
                Ok(Some(Frame {
                    node: index,
                    content,
                    start: content_start,
                }))
            }
        }
    }

    /// Appends a node, growing the index geometrically within both budgets.
    fn push_node(&mut self, node: Node) -> Result<()> {
        if self.nodes.len() == self.nodes.capacity() {
            let room = (self.memory.remaining() / size_of::<Node>()).max(1);
            let additional = self
                .nodes
                .len()
                .max(8)
                .min(self.max_elements - self.nodes.len())
                .min(room);
            let at = self.position(node.encoded.start);
            self.memory
                .reserve(&mut self.nodes, additional)
                .map_err(|error| error.at(at))?;
        }
        self.nodes.push(node);
        Ok(())
    }

    fn push_frame(&mut self, frame: Frame<'a>) -> Result<()> {
        if self.frames.len() == self.frames.capacity() {
            let room = (self.memory.remaining() / size_of::<Frame<'a>>()).max(1);
            let additional = self.frames.len().max(4).min(room);
            let at = self.position(self.nodes[frame.node].encoded.start);
            self.memory
                .reserve(&mut self.frames, additional)
                .map_err(|error| error.at(at))?;
        }
        self.frames.push(frame);
        Ok(())
    }

    const fn position(&self, offset: usize) -> Position {
        Position::Buffer(self.base + offset)
    }
}

fn read_tag(cursor: &mut Cursor<'_>) -> Result<Tag> {
    let at = cursor.position();
    let first = cursor.u8()?;
    let class = match first >> CLASS_SHIFT {
        0 => Class::Universal,
        1 => Class::Application,
        2 => Class::Context,
        _ => Class::Private,
    };
    let form = if first & CONSTRUCTED_BIT == 0 {
        Form::Primitive
    } else {
        Form::Constructed
    };
    let low = first & LOW_NUMBER;
    if low != HIGH_NUMBER {
        return Ok(Tag::new(class, form, u32::from(low)));
    }
    let mut number = 0u32;
    loop {
        let octet = cursor.u8()?;
        // X.690 8.1.2.4.2 c: the first subsequent octet's seven bits are not all zero.
        if number == 0 && octet & SEVEN_BITS == 0 {
            return Err(Error::syntax("nonminimal ASN.1 tag").at(at));
        }
        number = number
            .checked_mul(1 << 7)
            .and_then(|number| number.checked_add(u32::from(octet & SEVEN_BITS)))
            .ok_or_else(|| Error::limit("ASN.1 tag number exceeds u32").at(at))?;
        if octet & CONTINUATION == 0 {
            break;
        }
    }
    // X.690 8.1.2.2: numbers up to 30 use the single-octet form.
    if number < u32::from(HIGH_NUMBER) {
        return Err(Error::syntax("nonminimal ASN.1 tag").at(at));
    }
    Ok(Tag::new(class, form, number))
}

/// Reads a length; `None` is BER's indefinite form.
fn read_length(
    cursor: &mut Cursor<'_>,
    encoding: Encoding,
    diagnostics: &mut Diagnostics,
) -> Result<Option<usize>> {
    let at = cursor.position();
    let first = cursor.u8()?;
    if first & CONTINUATION == 0 {
        return Ok(Some(usize::from(first)));
    }
    let count = usize::from(first & SEVEN_BITS);
    if count == 0 {
        deviation(
            encoding,
            diagnostics,
            repairs::BER_INDEFINITE,
            at,
            "indefinite ASN.1 length is not DER",
        )?;
        return Ok(None);
    }
    if first == RESERVED_LENGTH {
        return Err(Error::syntax("reserved ASN.1 length").at(at));
    }
    if count > size_of::<usize>() {
        return Err(Error::limit("ASN.1 length exceeds address space").at(at));
    }
    let octets = cursor.take(count)?;
    // At most size_of::<usize>() octets, so the shifts cannot overflow.
    let length = octets
        .iter()
        .fold(0usize, |length, &octet| (length << 8) | usize::from(octet));
    if octets.first() == Some(&0) || length < LONG_FORM_MINIMUM {
        deviation(
            encoding,
            diagnostics,
            repairs::BER_LENGTH,
            at,
            "nonminimal ASN.1 length",
        )?;
    }
    Ok(Some(length))
}

/// A deviation from DER: a tagged error in DER, a reported repair in BER.
fn deviation(
    encoding: Encoding,
    diagnostics: &mut Diagnostics,
    repair: Repair,
    at: Position,
    message: &'static str,
) -> Result<()> {
    match encoding {
        Encoding::Ber => diagnostics.repair(Diagnostic::new(repair, message).at(at)),
        Encoding::Der => Err(Error::syntax(message).at(at).with_repair(repair)),
    }
}

fn validate_constructed(
    tag: Tag,
    at: Position,
    encoding: Encoding,
    diagnostics: &mut Diagnostics,
) -> Result<()> {
    if tag.class != Class::Universal {
        return Ok(());
    }
    match tag.number {
        universal::SEQUENCE | universal::SET => Ok(()),
        universal::OCTET_STRING => deviation(
            encoding,
            diagnostics,
            repairs::BER_OCTETS,
            at,
            "constructed OCTET STRING is not DER",
        ),
        _ => Err(Error::unsupported(
            features::CONSTRUCTED_TYPE,
            "unsupported constructed universal type",
        )
        .at(at)),
    }
}
