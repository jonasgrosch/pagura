//! Content compilation from immutable objects and optional in-memory form values.

mod annotations;
mod annotations_filled;
#[cfg(test)]
mod annotations_filled_tests;
mod forms;
pub(crate) mod images;
mod masks;
mod path;
mod pattern;
mod session;
mod shading;
mod state;
pub(crate) mod text;

use crate::color::{self, Color};
use crate::display::{Clip, Command, DisplayList, Mark, PathCommand};
use crate::features;
use crate::operators::{MAX_OPERANDS, Operation, Operator, Token, token};
use crate::optional::Visibility;
use pagura_core::syntax::{Origin, Parser};
use pagura_core::{Dictionary, Error, Feature, Name, ObjectId, Position, Rect, Result, Value};
use pagura_document::{Document, IndirectObject, Page};
use pagura_raster::{FillRule, LineCap, LineJoin, PathBuilder, Point, Stroke, Transform};
use session::Session;
use state::Graphics;
use std::sync::Arc;

pub(crate) fn repair(document: &Document, diagnostic: pagura_core::Diagnostic) -> Result<()> {
    let mut diagnostics = pagura_core::Diagnostics::new(document.recovery());
    diagnostics.repair(diagnostic)?;
    document.report(diagnostics.take());
    Ok(())
}

pub use annotations_filled::compile_filled_page;
pub(crate) use annotations_filled::compile_filled_shared;
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
    compile_page_with_provider(document, page, None)
}

/// Compiles a page, taking fonts the document does not embed from
/// `provider` (ADR 0009). Use [`crate::RenderContext`] to share fonts and the
/// optional-content configuration across pages.
///
/// # Errors
/// As [`compile_page`], including errors from the provider.
pub fn compile_page_with_provider(
    document: &Document,
    page: &Page,
    provider: Option<&dyn pagura_text::Provider>,
) -> Result<DisplayList> {
    Shared::standalone(document, provider, |shared| {
        compile_shared(document, page, shared)
    })
}

pub(crate) fn compile_shared(
    document: &Document,
    page: &Page,
    shared: &Shared<'_>,
) -> Result<DisplayList> {
    compile(document, page, shared, None, None).map(|(list, _)| list)
}

/// Document-bound resources that compilations borrow: the provider, the font
/// cache and the default optional-content configuration (ADR 0009). A
/// [`crate::RenderContext`] keeps them across pages.
pub(crate) struct Shared<'a> {
    pub(crate) provider: Option<&'a dyn pagura_text::Provider>,
    pub(crate) fonts: &'a std::sync::Mutex<text::cache::FontCache>,
    pub(crate) visibility: &'a Visibility,
}

impl Shared<'_> {
    /// Runs `compile` with resources that live for this one call.
    pub(crate) fn standalone<T>(
        document: &Document,
        provider: Option<&dyn pagura_text::Provider>,
        compile: impl FnOnce(&Shared<'_>) -> Result<T>,
    ) -> Result<T> {
        let fonts = std::sync::Mutex::new(text::cache::FontCache::new(document.limits()));
        let visibility = Visibility::new(document)?;
        compile(&Shared {
            provider,
            fonts: &fonts,
            visibility: &visibility,
        })
    }
}

/// Interprets a page and its annotations in one session, with in-memory
/// form values when `filler` is given, also collecting positioned text when
/// `text` is given.
fn compile(
    document: &Document,
    page: &Page,
    shared: &Shared<'_>,
    filler: Option<&pagura_document::forms::FormFiller<'_>>,
    text: Option<text::extract::Output>,
) -> Result<(DisplayList, Option<text::extract::Output>)> {
    let session = Session::new(document, page, shared, filler, text);
    let mut compiler = Compiler::new(&session);
    let group = compiler.page().map_err(|e| e.in_object(page.id()))?;
    let mut list = DisplayList::new(page, document.limits());
    list.group = group;
    list.commands = compiler.commands;
    list.print_intent = crate::overprint::print_intent(document, page);
    Ok((list, session.text.map(std::cell::RefCell::into_inner)))
}

/// One content context: the page, or a form, soft mask, tiling cell or
/// Type 3 glyph interpreted inside it (see [`session::Child`]).
struct Compiler<'s> {
    session: &'s Session<'s>,
    document: &'s Document,
    commands: Vec<Command>,
    state: Graphics,
    saved: Vec<Graphics>,
    path: PathBuilder,
    pending_clip: Option<FillRule>,
    operands: Vec<Value>,
    compatibility: usize,
    marked: Vec<bool>,
    visible: bool,
    initial_ctm: Transform,
    initial_state: Graphics,
    stencil_pattern: bool,
    position: Position,
    forms: Vec<ObjectId>,
    /// The open text object (9.4.1): not graphics state, so `q`/`Q` and
    /// child contexts never copy it.
    text_object: Option<text::TextObject>,
    /// The display-list index of the glyph run this context is extending
    /// (9.3.8); any operator other than text showing or text state ends it.
    text_run: Option<usize>,
    procedure: Option<text::type3::Procedure>,
    extraction: Option<text::extract::Scope>,
    /// Coloured patterns and shading patterns already selected in this
    /// context: their cells and transforms depend only on its initial state.
    patterns: std::collections::BTreeMap<ObjectId, Arc<crate::pattern::Pattern>>,
}

impl Compiler<'_> {
    /// Interprets the page's content and annotations; returns the page
    /// group's attributes (11.4.7).
    fn page(&mut self) -> Result<crate::transparency::Attributes> {
        let page = self.session.page;
        let context = self.session.colors();
        let group =
            crate::transparency::Attributes::parse(&context, page.dictionary(), page.resources())?
                .unwrap_or_default();
        self.state.fill.color =
            Color::new(color::named(&context, page.resources(), b"DeviceGray")?);
        self.state.stroke.color = self.state.fill.color.clone();
        let streams = self.document.content_streams(page)?;
        self.sequence(&streams, page.resources())?;
        self.annotations(page)?;
        Ok(group)
    }

    fn sequence(&mut self, streams: &[&IndirectObject], resources: &Dictionary) -> Result<()> {
        let mut bytes = Vec::new();
        let mut origins = Vec::new();
        let mut held = self.session.hold(0)?;
        for stream in streams {
            let data = self.document.stream_data(stream)?;
            let length = data.len().saturating_add(1);
            held.add(length)?;
            bytes
                .try_reserve(length)
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
        // Patterns selected in this content start from this state (8.7.3.1).
        self.initial_state = self.state.clone();
        let mut parser = Parser::new(bytes, Origin::Buffer, self.document.limits());
        let mut stream_index = 0;
        while parser.peek().is_some() {
            self.session
                .charge_operations(1)
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
            let result = token(&mut parser).and_then(|token| match token {
                Some(Token::Operand(value)) => self.operand(value),
                Some(Token::Operator(b"BI")) => self.inline_image(&mut parser, resources),
                Some(Token::Operator(word)) => self.operator(word, resources),
                None => Ok(()),
            });
            result.map_err(|e| stream_error(e, offset, origins))?;
        }
        if !self.operands.is_empty()
            || self.compatibility != 0
            || !self.marked.is_empty()
            || !self.path.is_empty()
            || self.pending_clip.is_some()
            || self.text_object.is_some()
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
            )
            .position;
            repair(
                self.document,
                pagura_core::Diagnostic::new(
                    crate::repairs::GRAPHICS_UNCLOSED,
                    "unmatched q states discarded at end of content sequence (7.8.2, 8.4.2)",
                )
                .at(position)
                .in_object_opt(position.object()),
            )?;
            self.saved.clear();
        }
        Ok(())
    }

    fn operand(&mut self, value: Value) -> Result<()> {
        if self.operands.len() >= MAX_OPERANDS {
            return Err(Error::limit("content operand budget exceeded"));
        }
        no_references(&value)?;
        self.operands.push(value);
        Ok(())
    }

    fn operator(&mut self, word: &[u8], resources: &Dictionary) -> Result<()> {
        let parsed = Operator::parse(word);
        // A glyph run continues only across text-showing and text-state
        // operators; any other operator ends it.
        if !parsed.is_some_and(Operator::keeps_text_run) {
            self.text_run = None;
        }
        let Some(operator) = parsed else {
            if self.compatibility > 0 {
                self.operands.clear();
                return Ok(());
            }
            return Err(Error::syntax("unknown content operator"));
        };
        let operands = std::mem::take(&mut self.operands);
        let result = operator.decode(&operands).and_then(|operation| {
            if self.glyph_ignores(operator)?
                || self.pattern_ignores(operator, operation, resources)?
            {
                Ok(())
            } else {
                self.execute(operation, resources)
            }
        });
        self.operands = operands;
        self.operands.clear();
        if self.path.len() > self.document.limits().path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        result
    }

    fn repair(&self, tag: pagura_core::Repair, message: &'static str) -> Result<()> {
        repair(
            self.document,
            pagura_core::Diagnostic::new(tag, message)
                .at(self.position)
                .in_object_opt(self.position.object()),
        )
    }

    fn execute(&mut self, operation: Operation<'_>, resources: &Dictionary) -> Result<()> {
        match operation {
            Operation::Save => self.save_graphics()?,
            Operation::Restore => self.restore_graphics()?,
            Operation::Concat(matrix) => {
                let matrix = matrix.then(self.state.ctm);
                if !matrix.is_finite() {
                    return Err(Error::syntax("CTM arithmetic overflow"));
                }
                self.state.ctm = matrix;
            }
            Operation::Parameter(parameter) => self.parameter(parameter)?,
            Operation::Construct(construct) => {
                if self.pending_clip.is_some() {
                    self.repair(
                        crate::repairs::CLIP_ORDER,
                        "construction follows clipping operator",
                    )?;
                }
                let pending = self.pending_clip.take();
                let result = self.construct(construct);
                self.pending_clip = pending;
                result?;
            }
            Operation::Paint(painting) => {
                if self.path.is_empty() {
                    self.repair(
                        crate::repairs::EMPTY_PATH,
                        "painting requires a current path",
                    )?;
                    self.pending_clip = None;
                } else {
                    self.paint(painting)?;
                }
            }
            Operation::Clip(rule) => {
                if self.pending_clip.is_some() {
                    return Err(Error::syntax("multiple clipping operators for one path"));
                }
                self.pending_clip = Some(rule);
            }
            Operation::Color(target, color) => self.color(target, color, resources)?,
            Operation::GraphicsState(name) => self.graphics_state(name, resources)?,
            Operation::XObject(name) => self.xobject(name, resources)?,
            Operation::Text(text) => self.text_operator(text, resources)?,
            Operation::Glyph(metrics) => self.glyph_metrics(metrics)?,
            Operation::BeginCompatibility => {
                self.compatibility = nested(self.compatibility, self.document.limits().depth)?;
            }
            Operation::EndCompatibility => {
                self.compatibility = self
                    .compatibility
                    .checked_sub(1)
                    .ok_or(Error::syntax("EX without BX"))?;
            }
            Operation::BeginMarked { tag, properties } => {
                self.begin_marked(tag, properties, resources)?;
            }
            Operation::EndMarked => {
                self.visible = self
                    .marked
                    .pop()
                    .ok_or(Error::syntax("EMC without marked content"))?;
                self.extract_end()?;
            }
            Operation::MarkPoint => {}
            Operation::ImageDelimiter => {
                return Err(Error::syntax("inline image delimiter outside image"));
            }
            Operation::Shade(name) => self.shading(name, resources)?,
        }
        Ok(())
    }
    fn begin_marked(
        &mut self,
        tag: &Name,
        properties: Option<&Value>,
        resources: &Dictionary,
    ) -> Result<()> {
        if self.marked.len() >= self.document.limits().depth {
            return Err(Error::limit("marked content nesting exceeded"));
        }
        let mut visible = true;
        if tag == b"OC"
            && let Some(properties) = properties
        {
            let properties = if let Some(name) = properties.as_name() {
                resource(self.document, resources, b"Properties", name.as_bytes())?
            } else {
                properties
            };
            visible = self.session.visibility.marked(self.document, properties)?;
        }
        self.marked.push(self.visible);
        self.visible &= visible;
        self.extract_begin(tag, properties, resources)?;
        Ok(())
    }
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
            for (_, value) in values {
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
    use super::compile_page;
    use crate::repairs as tags;
    use pagura_core::Recovery;
    use pagura_document::{Document, OpenOptions};

    #[test]
    fn content_repairs_are_reported_and_strictly_rejected() {
        for (name, tag) in [
            ("repair-empty-stroke", crate::repairs::EMPTY_PATH),
            ("repair-empty-clip", crate::repairs::EMPTY_PATH),
            ("repair-clip-construction", crate::repairs::CLIP_ORDER),
            ("repair-colour-Separation-fill", tags::COLOR_OPERATOR),
            ("repair-colour-DeviceN-stroke", tags::COLOR_OPERATOR),
            ("repair-icc-colour", tags::COLOR_OPERATOR),
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
