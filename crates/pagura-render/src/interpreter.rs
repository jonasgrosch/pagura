//! Content compilation. The only inputs are immutable document objects.

mod forms;
mod path;
mod state;

use crate::color::{self, Color};
use crate::display::{Clip, Command, DisplayList, Mark};
use crate::features;
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Dictionary, Error, Feature, ObjectId, Position, Rect, Result, Value};
use pagura_document::{Document, IndirectObject, Page};
use pagura_raster::{FillRule, LineCap, LineJoin, PathBuilder, Point, Stroke, Transform};
use std::sync::Arc;

/// Compiles a page into the vector display list (7.8.2; 8.4–8.6; ADR 0004).
/// All content streams are one token sequence, including compound operands.
///
/// # Errors
/// Returns positioned syntax errors, resource-limit errors and stable
/// `Unsupported` tags. No partial list is returned on failure. Text, images,
/// annotation appearances and nontrivial transparency currently remain unsupported.
pub fn compile_page(document: &Document, page: &Page) -> Result<DisplayList> {
    let mut compiler = Compiler::new(document, page)?;
    compiler.page(page).map_err(|e| e.in_object(page.id()))?;
    Ok(compiler.list)
}

#[derive(Debug, Clone)]
struct Graphics {
    ctm: Transform,
    fill: Color,
    stroke_color: Color,
    stroke: Stroke,
    clip: Option<Arc<Clip>>,
    clip_depth: usize,
    intent: Vec<u8>,
    flatness: f64,
    stroke_adjustment: bool,
    transfer: Option<crate::functions::Transfer>,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            ctm: Transform::IDENTITY,
            fill: Color::default(),
            stroke_color: Color::default(),
            stroke: Stroke::default(),
            clip: None,
            clip_depth: 0,
            intent: b"RelativeColorimetric".to_vec(),
            flatness: 1.0,
            stroke_adjustment: false,
            transfer: None,
        }
    }
}

struct Compiler<'a> {
    document: &'a Document,
    list: DisplayList,
    state: Graphics,
    saved: Vec<Graphics>,
    path: PathBuilder,
    pending_clip: Option<FillRule>,
    operands: Vec<Value>,
    compatibility: usize,
    marked: Vec<bool>,
    visible: bool,
    visibility: Arc<crate::optional::Visibility>,
    operations: usize,
    forms: Vec<ObjectId>,
}

impl<'a> Compiler<'a> {
    fn new(document: &'a Document, page: &Page) -> Result<Self> {
        Ok(Self {
            document,
            list: DisplayList::new(page, document.limits()),
            state: Graphics::default(),
            saved: Vec::new(),
            path: PathBuilder::new(),
            pending_clip: None,
            operands: Vec::new(),
            compatibility: 0,
            marked: Vec::new(),
            visible: true,
            visibility: Arc::new(crate::optional::Visibility::new(document)?),
            operations: 0,
            forms: Vec::new(),
        })
    }

    fn page(&mut self, page: &Page) -> Result<()> {
        group(self.document, page.dictionary(), page.resources())?;
        self.state.fill = Color::new(color::named(
            self.document,
            page.resources(),
            b"DeviceGray",
        )?);
        self.state.stroke_color = self.state.fill.clone();
        if let Some(annotations) = page.dictionary().get(b"Annots") {
            let annotations = self
                .document
                .resolve(annotations)?
                .as_array()
                .ok_or(Error::syntax("Annots must be an array"))?;
            for value in annotations {
                annotation(self.document, value)?;
            }
        }
        let streams = self.document.content_streams(page)?;
        self.sequence(&streams, page.resources())
    }

    fn sequence(&mut self, streams: &[&IndirectObject], resources: &Dictionary) -> Result<()> {
        let mut bytes = Vec::new();
        let mut origins = Vec::new();
        for stream in streams {
            let data = self.document.stream_data(stream)?;
            let end = bytes
                .len()
                .checked_add(data.len())
                .and_then(|n| n.checked_add(1))
                .filter(|&n| n <= self.document.limits().decoded_bytes)
                .ok_or(Error::limit("content sequence byte budget exceeded"))?;
            bytes
                .try_reserve(end - bytes.len())
                .map_err(|_| Error::limit("content sequence allocation failed"))?;
            origins.push((bytes.len(), stream.id()));
            bytes.extend_from_slice(&data);
            bytes.push(b'\n');
        }
        let mut parser = Parser::new(&bytes, Origin::Buffer, self.document.limits());
        while let Some(byte) = parser.peek() {
            self.operations = self
                .operations
                .checked_add(1)
                .filter(|&n| n <= self.document.limits().operations)
                .ok_or(Error::limit("content token budget exceeded"))
                .map_err(|e| stream_error(e, parser.offset(), &origins))?;
            let offset = parser.offset();
            let result = if matches!(
                byte,
                b'/' | b'(' | b'[' | b'<' | b'+' | b'-' | b'.' | b'0'..=b'9'
            ) {
                parser.parse().and_then(|value| self.operand(value))
            } else {
                parser.word().and_then(|word| match word {
                    b"true" => self.operand(Value::Boolean(true)),
                    b"false" => self.operand(Value::Boolean(false)),
                    b"null" => self.operand(Value::Null),
                    _ => self.operator(word, resources),
                })
            };
            result.map_err(|e| stream_error(e, offset, &origins))?;
        }
        if !self.operands.is_empty()
            || !self.saved.is_empty()
            || self.compatibility != 0
            || !self.marked.is_empty()
            || !self.path.is_empty()
            || self.pending_clip.is_some()
        {
            return Err(stream_error(
                Error::syntax("content sequence ends with unfinished state"),
                bytes.len().saturating_sub(1),
                &origins,
            ));
        }
        Ok(())
    }

    fn operand(&mut self, value: Value) -> Result<()> {
        if self.operands.len() >= 32 {
            return Err(Error::limit("content operand budget exceeded"));
        }
        no_references(&value)?;
        self.operands.push(value);
        Ok(())
    }

    fn operator(&mut self, word: &[u8], resources: &Dictionary) -> Result<()> {
        let tag = crate::operators::validate(word, &self.operands)?;
        let Some(tag) = tag else {
            if self.compatibility > 0 {
                self.operands.clear();
                return Ok(());
            }
            return Err(Error::syntax("unknown content operator"));
        };
        let operands = std::mem::take(&mut self.operands);
        let result = self.execute(word, &operands, resources, tag);
        self.operands = operands;
        self.operands.clear();
        if self.path.len() > self.document.limits().path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        result
    }

    fn execute(
        &mut self,
        word: &[u8],
        v: &[Value],
        resources: &Dictionary,
        tag: Feature,
    ) -> Result<()> {
        match word {
            b"q" => {
                if self.saved.len() >= self.document.limits().depth {
                    return Err(Error::limit("graphics state nesting exceeded"));
                }
                self.saved.push(self.state.clone());
            }
            b"Q" => {
                self.state = self
                    .saved
                    .pop()
                    .ok_or(Error::syntax("Q without matching q"))?;
            }
            b"cm" => {
                let matrix = matrix(v)?.then(self.state.ctm);
                if !matrix.is_finite() {
                    return Err(Error::syntax("CTM arithmetic overflow"));
                }
                self.state.ctm = matrix;
            }
            b"w" | b"J" | b"j" | b"M" | b"d" | b"i" | b"ri" => self.parameter(word, v)?,
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => self.construct(word, v)?,
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                self.paint(word)?;
            }
            b"W" | b"W*" => {
                if self.pending_clip.is_some() {
                    return Err(Error::syntax("multiple clipping operators for one path"));
                }
                self.pending_clip = Some(if word == b"W" {
                    FillRule::NonZero
                } else {
                    FillRule::EvenOdd
                });
            }
            b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" | b"CS" | b"cs" | b"SC" | b"SCN" | b"sc"
            | b"scn" => self.color(word, v, resources)?,
            b"gs" => self.extgstate(v, resources)?,
            b"Do" => self.form(v, resources)?,
            b"BX" => {
                self.compatibility = nested(self.compatibility, self.document.limits().depth)?;
            }
            b"EX" => {
                self.compatibility = self
                    .compatibility
                    .checked_sub(1)
                    .ok_or(Error::syntax("EX without BX"))?;
            }
            b"BMC" | b"BDC" => {
                if self.marked.len() >= self.document.limits().depth {
                    return Err(Error::limit("marked content nesting exceeded"));
                }
                let mut visible = true;
                if word == b"BDC"
                    && v.first()
                        .and_then(Value::as_name)
                        .is_some_and(|n| n == b"OC")
                {
                    let properties = v
                        .get(1)
                        .ok_or(Error::syntax("missing optional content properties"))?;
                    let properties = if let Some(name) = properties.as_name() {
                        resource(self.document, resources, b"Properties", name.as_bytes())?
                    } else {
                        properties
                    };
                    visible = self.visibility.visible(self.document, properties)?;
                }
                self.marked.push(self.visible);
                self.visible &= visible;
            }
            b"EMC" => {
                self.visible = self
                    .marked
                    .pop()
                    .ok_or(Error::syntax("EMC without marked content"))?;
            }
            b"MP" | b"DP" => {}
            b"BI" => return Err(unsupported(features::IMAGE_INLINE)),
            b"sh" => return Err(unsupported(features::SHADING_PAINT)),
            _ => {
                return Err(Error::unsupported(
                    tag,
                    "content operator is not implemented",
                ));
            }
        }
        Ok(())
    }
}

fn number(v: &[Value], index: usize) -> Result<f64> {
    v.get(index)
        .and_then(Value::as_number)
        .ok_or(Error::syntax("numeric operand expected"))
}

fn integer(v: &[Value], index: usize) -> Result<i64> {
    v.get(index)
        .and_then(Value::as_integer)
        .ok_or(Error::syntax("integer operand expected"))
}

fn point(v: &[Value], index: usize) -> Result<Point> {
    Ok(Point::new(number(v, index)?, number(v, index + 1)?))
}

fn matrix(v: &[Value]) -> Result<Transform> {
    if v.len() != 6 {
        return Err(Error::syntax("matrix must have six numbers"));
    }
    Ok(Transform::new(
        number(v, 0)?,
        number(v, 1)?,
        number(v, 2)?,
        number(v, 3)?,
        number(v, 4)?,
        number(v, 5)?,
    ))
}

fn nested(current: usize, maximum: usize) -> Result<usize> {
    current
        .checked_add(1)
        .filter(|&n| n <= maximum)
        .ok_or(Error::limit("content nesting budget exceeded"))
}

fn no_references(value: &Value) -> Result<()> {
    match value {
        Value::Reference(_) => return Err(Error::syntax("indirect reference in content operand")),
        Value::Array(values) => {
            for value in values {
                no_references(value)?;
            }
        }
        Value::Dictionary(values) => {
            for (_, value) in values.iter() {
                no_references(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn finite(point: Point) -> Result<()> {
    if !point.x.is_finite() || !point.y.is_finite() {
        return Err(Error::syntax("path arithmetic overflow"));
    }
    Ok(())
}

fn rendering_intent(value: &Value) -> Result<Vec<u8>> {
    let name = value
        .as_name()
        .ok_or(Error::syntax("rendering intent must be a name"))?;
    if !matches!(
        name.as_bytes(),
        b"AbsoluteColorimetric" | b"RelativeColorimetric" | b"Saturation" | b"Perceptual"
    ) {
        return Err(Error::syntax("invalid rendering intent"));
    }
    Ok(name.as_bytes().to_vec())
}

fn resource<'a>(
    document: &'a Document,
    resources: &'a Dictionary,
    kind: &[u8],
    name: &[u8],
) -> Result<&'a Value> {
    resources
        .get(kind)
        .map(|v| document.resolve_dictionary(v))
        .transpose()?
        .and_then(|d| d.get(name))
        .ok_or(Error::syntax("undefined content resource"))
}

fn unsupported(tag: Feature) -> Error {
    Error::unsupported(tag, "rendering feature is not implemented")
}

/// With alpha=1, Normal blending and no soft mask, the source-over equation
/// (11.3–11.4) yields the source colour independently of backdrop/isolation.
/// A non-knockout group can therefore be painted directly once every mark is
/// validated as opaque. For a DeviceRGB blend space on an RGB output device
/// there is no intervening colour conversion; ICCBased with an RGB alternate
/// has the same fallback representation (8.6.5.5). Gray/CMYK and calibrated
/// blend spaces require a conversion round trip and are excluded (11.4.7).
fn group(document: &Document, dictionary: &Dictionary, resources: &Dictionary) -> Result<()> {
    let Some(value) = dictionary.get(b"Group") else {
        return Ok(());
    };
    let dictionary = document.resolve_dictionary(value)?;
    if !dictionary.is_name(b"S", b"Transparency") {
        return Err(unsupported(features::TRANSPARENCY_GROUP));
    }
    if let Some(value) = dictionary.get(b"K") {
        let knockout = document
            .resolve(value)?
            .as_bool()
            .ok_or(Error::syntax("group K must be boolean"))?;
        if knockout {
            return Err(unsupported(features::TRANSPARENCY_KNOCKOUT));
        }
    }
    if let Some(isolated) = dictionary.get(b"I") {
        document
            .resolve(isolated)?
            .as_bool()
            .ok_or(Error::syntax("group I must be boolean"))?;
    }
    if let Some(space) = dictionary.get(b"CS")
        && !color::rgb_blending(document, resources, space)?
    {
        return Err(unsupported(features::TRANSPARENCY_GROUP_COLORSPACE));
    }
    Ok(())
}

fn stream_error(mut error: Error, fallback: usize, origins: &[(usize, ObjectId)]) -> Error {
    let offset = match error.position {
        Position::Buffer(n) => n,
        _ => fallback,
    };
    if matches!(error.position, Position::Unknown | Position::Buffer(_))
        && let Some(&(start, object)) = origins.iter().rev().find(|(start, _)| *start <= offset)
    {
        error.position = Position::Stream {
            object,
            offset: offset - start,
        };
        error = error.in_object(object);
    }
    error
}

fn annotation(document: &Document, value: &Value) -> Result<()> {
    let dictionary = document.resolve_dictionary(value)?;
    let flags = dictionary
        .get(b"F")
        .map(|v| {
            document
                .resolve(v)?
                .as_integer()
                .ok_or(Error::syntax("annotation F must be integer"))
        })
        .transpose()?
        .unwrap_or(0);
    // Screen rendering: Hidden and NoView suppress normal appearances (12.5.3).
    if flags & (2 | 32) != 0 {
        return Ok(());
    }
    let Some(appearance) = dictionary.get(b"AP") else {
        return Ok(());
    };
    let appearance = document.resolve_dictionary(appearance)?;
    let Some(normal) = appearance.get(b"N") else {
        return Ok(());
    };
    let normal = if let Some(id) = normal.as_reference()
        && document.object(id)?.is_stream()
    {
        normal
    } else {
        let states = document.resolve_dictionary(normal)?;
        if states.is_empty() {
            return Ok(());
        }
        let state = dictionary
            .get_name(b"AS")
            .ok_or(Error::syntax("annotation has no appearance state"))?;
        let Some(normal) = states.get(state.as_bytes()) else {
            return Ok(());
        };
        normal
    };
    let id = normal.as_reference().ok_or(Error::syntax(
        "normal appearance must be a stream reference",
    ))?;
    if !document.object(id)?.is_stream() {
        return Err(Error::syntax("normal appearance must be a stream"));
    }
    let mut error = unsupported(features::ANNOTATION_APPEARANCE);
    if let Some(id) = value.as_reference() {
        error = error.in_object(id);
    }
    Err(error)
}
