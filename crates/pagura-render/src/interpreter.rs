//! Content compilation from immutable objects and optional in-memory form values.

mod annotation_shapes;
mod annotation_widgets;
mod annotations;
mod annotations_filled;
#[cfg(test)]
mod annotations_filled_tests;
mod forms;
pub(crate) mod images;
mod masks;
mod path;
mod pattern_inline;
mod patterns;
mod shadings;
mod state;
pub(crate) mod text;

use crate::color::{self, Color};
use crate::display::{Clip, Command, DisplayList, Mark, PathCommand};
use crate::features;
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Dictionary, Error, Feature, ObjectId, Position, Rect, Result, Value};
use pagura_document::{Document, IndirectObject, Page};
use pagura_raster::{FillRule, LineCap, LineJoin, PathBuilder, Point, Stroke, Transform};
use std::sync::Arc;

/// Stable reported content repair and information tags (ADR 0003).
pub(crate) mod tags {
    use pagura_core::Repair;

    /// Information, not a repair: an untouched field keeps its valid stored AP
    /// because generating a replacement is unsupported.
    pub const APPEARANCE_KEPT: &str = "form.appearance-kept";
    /// Table 73: numeric SC/sc operands are interpreted as SCN/scn only
    /// when their count and types fit the selected non-pattern space.
    pub const COLOUR_OPERATOR: Repair = Repair::new("content.colour-operator");
    /// 8.5.3: an undefined path paints nothing; any pending clip is discarded.
    pub const EMPTY_PATH: Repair = Repair::new("content.empty-path");
    /// 8.5.4: defer clipping until painting, including subsequent construction.
    pub const CLIP_ORDER: Repair = Repair::new("content.clip-order");
    /// 8.6.6.3: ignore bytes beyond the complete, declared palette.
    pub const INDEXED_EXCESS: Repair = Repair::new("content.indexed-excess");
    /// Table 98: ignore optional-content structures without a default config.
    pub const OPTIONAL_CONFIG: Repair = Repair::new("content.optional-config");
    /// Table 98: ignore optional-content structures without a group registry.
    pub const OPTIONAL_GROUPS: Repair = Repair::new("content.optional-groups");
    /// 8.11.4.2: ignore optional structures when required OCProperties is absent.
    pub const OPTIONAL_PROPERTIES: Repair = Repair::new("content.optional-properties");
    /// 8.11.4.3: use the initial ON state for an unregistered, valid OCG.
    /// This also applies under OCMD negation; readers differ on this policy.
    pub const OPTIONAL_GROUP: Repair = Repair::new("content.optional-group");
    /// 9.3.6: retain accumulated glyph clips through ET after a nonclipping reset.
    /// Most readers keep the clip when recovering this prohibited transition.
    pub const TEXT_CLIP_RESET: Repair = Repair::new("text.clip-mode-reset");
    /// 7.9.5; 12.5.5: skip an appearance with a zero-sized source or target
    /// rectangle. Most readers paint nothing where fitting has no area.
    pub const APPEARANCE_EMPTY: Repair = Repair::new("annotation.appearance-empty");
    /// 7.8.2; 8.4.2: discard unmatched saved graphics states at the end of a
    /// content sequence, after all painting and text objects have finished.
    pub const GRAPHICS_UNCLOSED: Repair = Repair::new("content.graphics-unclosed");
    /// 9.6.4, Table 111: retain the horizontal Widths advance when d0/d1 disagrees.
    pub const TYPE3_WIDTH: Repair = Repair::new("font.type3-width");
    /// 8.7.3.2, Table 73: coloured patterns require only a name; discard extra
    /// numeric operands supplied with an underlying pattern colour space.
    pub const PATTERN_UNDERLYING: Repair = Repair::new("content.pattern-underlying");
}

pub(crate) fn repair(document: &Document, diagnostic: pagura_core::Diagnostic) -> Result<()> {
    let mut diagnostics = pagura_core::Diagnostics::new(document.recovery());
    diagnostics.repair(diagnostic)?;
    document.report(diagnostics.take());
    Ok(())
}

pub(crate) use annotations_filled::compile_filled_cached;
pub use annotations_filled::compile_filled_page;
pub(crate) use text::extract::compile_with_text;
pub use text::extract::{compile_page_with_text, text_page};

/// Compiles a page into the vector display list (7.8.2; 8.4–8.6; ADR 0004).
/// All content streams are one token sequence, including compound operands.
/// This uses stored appearances and ignores NeedAppearances. Use
/// [`compile_filled_page`] with an empty filler to honor NeedAppearances.
///
/// # Errors
/// Returns positioned syntax errors, resource-limit errors and stable
/// `Unsupported` tags. No partial list is returned on failure.
pub fn compile_page(document: &Document, page: &Page) -> Result<DisplayList> {
    let fonts = std::sync::Mutex::new(text::cache::FontCache::new(document.limits()));
    compile_cached(document, page, None, &fonts)
}

/// Compiles a page using external font resources supplied explicitly (ADR 0009).
/// Use [`crate::RenderContext`] to share fonts across multiple pages.
///
/// # Errors
/// As [`compile_page`], including errors from the provider.
pub fn compile_page_with(
    document: &Document,
    page: &Page,
    provider: &dyn pagura_text::Provider,
) -> Result<DisplayList> {
    let fonts = std::sync::Mutex::new(text::cache::FontCache::new(document.limits()));
    compile_cached(document, page, Some(provider), &fonts)
}

pub(crate) fn compile_cached(
    document: &Document,
    page: &Page,
    provider: Option<&dyn pagura_text::Provider>,
    fonts: &std::sync::Mutex<text::cache::FontCache>,
) -> Result<DisplayList> {
    let mut compiler = Compiler::new(document, page)?;
    compiler.compilation = std::rc::Rc::new(Compilation {
        colors: crate::icc::Cache::default(),
        provider,
        fonts: Some(fonts),
        filler: None,
    });
    compiler.page(page).map_err(|e| e.in_object(page.id()))?;
    Ok(compiler.list)
}

// Resources shared by detached forms and masks within one compilation.
struct Compilation<'a> {
    filler: Option<&'a pagura_document::forms::FormFiller<'a>>,
    colors: crate::icc::Cache,
    provider: Option<&'a dyn pagura_text::Provider>,
    fonts: Option<&'a std::sync::Mutex<text::cache::FontCache>>,
}

#[derive(Debug, Clone)]
struct Graphics {
    text: text::TextState,
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
    composite: crate::transparency::Composite,
    stroke_alpha: f64,
    stroke_overprint: bool,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            text: text::TextState::default(),
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
            composite: crate::transparency::Composite::opaque(),
            stroke_alpha: 1.0,
            stroke_overprint: false,
        }
    }
}

struct Compiler<'a> {
    document: &'a Document,
    compilation: std::rc::Rc<Compilation<'a>>,
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
    marks: usize,
    initial_ctm: Transform,
    initial_state: Graphics,
    stencil_pattern: bool,
    position: Position,
    forms: Vec<ObjectId>,
    extraction: Option<text::extract::Extraction>,
}

impl<'a> Compiler<'a> {
    fn new(document: &'a Document, page: &Page) -> Result<Self> {
        Ok(Self {
            document,
            compilation: std::rc::Rc::new(Compilation {
                colors: crate::icc::Cache::default(),
                provider: None,
                fonts: None,
                filler: None,
            }),
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
            marks: 0,
            initial_ctm: Transform::IDENTITY,
            initial_state: Graphics::default(),
            stencil_pattern: false,
            position: Position::Unknown,
            forms: Vec::new(),
            extraction: None,
        })
    }

    fn page(&mut self, page: &Page) -> Result<()> {
        self.paint_charge(page.resources().retained_bytes(), 0)?;
        self.state.text.cache.borrow_mut().page_resources =
            Some(Arc::new(page.resources().clone()));
        let context = crate::icc::ColorContext::new(self.document, &self.compilation.colors);
        self.list.group =
            crate::transparency::Attributes::parse(&context, page.dictionary(), page.resources())?
                .unwrap_or_default();
        self.state.fill = Color::new(color::named(&context, page.resources(), b"DeviceGray")?);
        self.state.stroke_color = self.state.fill.clone();
        let streams = self.document.content_streams(page)?;
        self.sequence(&streams, page.resources())?;
        self.annotations(page)?;
        self.list.print_intent = crate::overprint::print_intent(self.document, page);
        Ok(())
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
        self.sequence_bytes(&bytes, &origins, resources)
    }

    fn sequence_bytes(
        &mut self,
        bytes: &[u8],
        origins: &[(usize, ObjectId)],
        resources: &Dictionary,
    ) -> Result<()> {
        // Pattern execution restores the parent's initial graphics state (8.7.3.1).
        self.state.composite.conversion.intent = crate::icc::Intent::from_name(&self.state.intent);
        self.initial_state = self.state.clone();
        let mut parser = Parser::new(bytes, Origin::Buffer, self.document.limits());
        let mut stream_index = 0;
        while let Some(byte) = parser.peek() {
            self.operations = self
                .operations
                .checked_add(1)
                .filter(|&n| n <= self.document.limits().operations)
                .ok_or(Error::limit("content token budget exceeded"))
                .map_err(|e| stream_error(e, parser.offset(), origins))?;
            let offset = parser.offset();
            while origins
                .get(stream_index + 1)
                .is_some_and(|(start, _)| *start <= offset)
            {
                stream_index += 1;
            }
            if let Some(&(start, object)) = origins.get(stream_index) {
                self.position = Position::Stream {
                    object,
                    offset: offset - start,
                };
            } else {
                self.position = Position::Buffer(offset);
            }
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
                    b"BI" => self.pattern_inline(&mut parser, resources),
                    _ => self.operator(word, resources),
                })
            };
            result.map_err(|e| stream_error(e, offset, origins))?;
        }
        if !self.operands.is_empty()
            || self.compatibility != 0
            || !self.marked.is_empty()
            || !self.path.is_empty()
            || self.pending_clip.is_some()
            || self.state.text.object.is_some()
        {
            return Err(stream_error(
                Error::syntax("content sequence ends with unfinished state"),
                bytes.len().saturating_sub(1),
                origins,
            ));
        }
        if !self.saved.is_empty() {
            let position = stream_error(
                Error::syntax("unclosed saved graphics states"),
                bytes.len().saturating_sub(1),
                origins,
            );
            repair(
                self.document,
                pagura_core::Diagnostic::new(
                    tags::GRAPHICS_UNCLOSED,
                    "unmatched q states discarded at end of content sequence (7.8.2, 8.4.2)",
                )
                .at(position.position)
                .in_object(match position.position {
                    Position::Stream { object, .. } => object,
                    _ => return Err(position),
                }),
            )?;
            self.saved.clear();
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
        if !matches!(
            word,
            b"Tj"
                | b"TJ"
                | b"'"
                | b"\""
                | b"Tc"
                | b"Tw"
                | b"Tz"
                | b"TL"
                | b"Ts"
                | b"Tf"
                | b"Tm"
                | b"Td"
                | b"TD"
                | b"T*"
        ) {
            self.state.text.run = None;
        }
        let tag = crate::operators::validate(word, &self.operands)?;
        let Some(_) = tag else {
            if self.compatibility > 0 {
                self.operands.clear();
                return Ok(());
            }
            return Err(Error::syntax("unknown content operator"));
        };
        let operands = std::mem::take(&mut self.operands);
        let result = self.glyph_operator(word).and_then(|ignore| {
            if ignore || self.pattern_ignores(word, &operands, resources)? {
                Ok(())
            } else {
                self.execute(word, &operands, resources)
            }
        });
        self.operands = operands;
        self.operands.clear();
        if self.path.len() > self.document.limits().path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        result?;
        self.paint_charge(0, 0)
    }

    fn repair(&self, tag: pagura_core::Repair, message: &'static str) -> Result<()> {
        let mut diagnostic = pagura_core::Diagnostic::new(tag, message).at(self.position);
        if let Position::Stream { object, .. } = self.position {
            diagnostic = diagnostic.in_object(object);
        }
        repair(self.document, diagnostic)
    }

    fn execute(&mut self, word: &[u8], v: &[Value], resources: &Dictionary) -> Result<()> {
        match word {
            b"q" => self.save_graphics()?,
            b"Q" => self.restore_graphics()?,
            b"cm" => {
                let matrix = matrix(v)?.then(self.state.ctm);
                if !matrix.is_finite() {
                    return Err(Error::syntax("CTM arithmetic overflow"));
                }
                self.state.ctm = matrix;
            }
            b"w" | b"J" | b"j" | b"M" | b"d" | b"i" | b"ri" => self.parameter(word, v)?,
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => {
                if self.pending_clip.is_some() {
                    self.repair(tags::CLIP_ORDER, "construction follows clipping operator")?;
                }
                let pending = self.pending_clip.take();
                let result = self.construct(word, v);
                self.pending_clip = pending;
                result?;
            }
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                if self.path.is_empty() {
                    self.repair(tags::EMPTY_PATH, "painting requires a current path")?;
                    self.pending_clip = None;
                } else {
                    self.paint(word)?;
                }
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
            | b"scn" => {
                self.color(word, v, resources)?;
                self.compile_pattern(word)?;
            }
            b"gs" => self.paint_gstate(v, resources)?,
            b"Do" => self.xobject(v, resources)?,
            b"BT" | b"ET" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tf" | b"Tr" | b"Ts" | b"Td"
            | b"TD" | b"Tm" | b"T*" | b"Tj" | b"TJ" | b"'" | b"\"" | b"d0" | b"d1" => {
                if self.extraction.is_some() {
                    self.text_operator::<true>(word, v, resources)?;
                } else {
                    self.text_operator::<false>(word, v, resources)?;
                }
            }
            b"BX" => {
                self.compatibility = nested(self.compatibility, self.document.limits().depth)?;
            }
            b"EX" => {
                self.compatibility = self
                    .compatibility
                    .checked_sub(1)
                    .ok_or(Error::syntax("EX without BX"))?;
            }
            b"BMC" | b"BDC" => self.begin_marked(word, v, resources)?,
            b"EMC" => {
                self.visible = self
                    .marked
                    .pop()
                    .ok_or(Error::syntax("EMC without marked content"))?;
                if self.extraction.is_some() {
                    self.extract_end()?;
                }
            }
            b"MP" | b"DP" => {}
            b"ID" | b"EI" => return Err(Error::syntax("inline image delimiter outside image")),
            b"sh" => self.shading(v, resources)?,
            _ => {
                return Err(Error::unsupported(
                    features::operator_tag(word),
                    "content operator is not implemented",
                ));
            }
        }
        Ok(())
    }
    fn begin_marked(&mut self, word: &[u8], v: &[Value], resources: &Dictionary) -> Result<()> {
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
            visible = self.visibility.marked(self.document, properties)?;
        }
        self.marked.push(self.visible);
        self.visible &= visible;
        if self.extraction.is_some() {
            self.extract_begin(word, v, resources)?;
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
    // ISO 8.6.5.8 Table 69 permits RelativeColorimetric for unknown intents.
    Ok(
        if matches!(
            name.as_bytes(),
            b"RelativeColorimetric" | b"AbsoluteColorimetric" | b"Perceptual" | b"Saturation"
        ) {
            name.as_bytes().to_vec()
        } else {
            b"RelativeColorimetric".to_vec()
        },
    )
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
    } else if error.position == Position::Unknown {
        error.position = Position::Buffer(offset);
    }
    error
}

#[cfg(test)]
mod recovery_tests {
    use super::{compile_page, tags};
    use pagura_core::Recovery;
    use pagura_document::{Document, OpenOptions};

    #[test]
    fn content_repairs_are_reported_and_strictly_rejected() {
        for (name, tag) in [
            ("repair-empty-stroke", tags::EMPTY_PATH),
            ("repair-empty-clip", tags::EMPTY_PATH),
            ("repair-clip-construction", tags::CLIP_ORDER),
            ("repair-colour-Separation-fill", tags::COLOUR_OPERATOR),
            ("repair-colour-DeviceN-stroke", tags::COLOUR_OPERATOR),
            ("repair-icc-colour", tags::COLOUR_OPERATOR),
            ("repair-indexed-string", tags::INDEXED_EXCESS),
            ("repair-indexed-stream", tags::INDEXED_EXCESS),
            ("repair-optional-config-AnyOff", tags::OPTIONAL_CONFIG),
            ("repair-optional-groups-AnyOn", tags::OPTIONAL_GROUPS),
            ("repair-optional-absent-AnyOff", tags::OPTIONAL_PROPERTIES),
            ("repair-optional-unregistered-AnyOff", tags::OPTIONAL_GROUP),
        ] {
            let bytes = pagura_testkit::fixture("content", &format!("{name}.pdf"));
            let original = bytes.clone();
            let repaired = Document::open(bytes.clone(), &OpenOptions::default()).unwrap();
            compile_page(&repaired, repaired.page(0).unwrap()).unwrap();
            let diagnostics = repaired.diagnostics();
            assert_eq!(diagnostics.len(), 1, "{name}: {diagnostics:?}");
            assert_eq!(diagnostics[0].repair(), Some(tag), "{name}");
            compile_page(&repaired, repaired.page(0).unwrap()).unwrap();
            assert_eq!(repaired.diagnostics(), diagnostics, "{name}");
            assert_eq!(repaired.original_bytes(), original);
            let strict = Document::open(
                bytes,
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let error = compile_page(&strict, strict.page(0).unwrap()).unwrap_err();
            assert_eq!(error.repair, Some(tag), "{name}: {error}");
            assert!(error.object.is_some(), "{name}: {error}");
            assert!(strict.diagnostics().is_empty(), "{name}");
        }
    }
}
