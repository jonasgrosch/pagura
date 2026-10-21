//! The graphics state machine (clauses 8–11, 12.5, 14.6): one content
//! sequence at a time, with nested sequences for forms, patterns, soft masks,
//! Type 3 glyphs and annotation appearances, emitting into a [`Device`].

mod annotations;
mod color;
mod emit;
mod extgstate;
mod form;
mod image;
mod marked;
mod params;
mod path;
mod pattern;
mod shading;
mod soft_mask;
mod state;
mod text;

use crate::device::{Device, Needs, PageInfo};
use crate::fonts::Fonts;
use crate::model::{Composite, MaskId, PatternId};
use crate::repairs;
use crate::resources::colorspace::Colors;
use crate::resources::optional::Visibility;
use emit::Lazy;
use pagura_content::{Content, Op, Reader};
use pagura_core::{Diagnostic, Dictionary, Error, Limits, Meter, ObjectId, Position, Result, Store};
use pagura_document::{Document, Page};
use pagura_raster::{FillRule, PathBuilder, Transform};
use state::Graphics;
use std::collections::HashMap;
use std::sync::Arc;
use text::{Procedure, TextObject};

/// What one interpretation shares across its content sequences.
pub(crate) struct Session<'a> {
    pub(crate) document: &'a Document,
    pub(crate) limits: Limits,
    pub(crate) needs: Needs,
    pub(crate) filler: Option<&'a pagura_document::forms::FormFiller<'a>>,
    pub(crate) visibility: Visibility,
    pub(crate) colors: Colors<'a>,
    pub(crate) fonts: Fonts<'a>,
    /// Content tokens, glyphs and inline-image entries (`operations`).
    pub(crate) tokens: Meter,
    /// The page's resources, for Type 3 fonts without their own.
    pub(crate) page_resources: Option<&'a Dictionary>,
    /// Glyph-name Unicode work of text extraction (`text_work`).
    pub(crate) name_work: usize,
    /// The composite state of implicit groups: opaque and Normal.
    pub(crate) opaque: Arc<Composite>,
    /// The next soft mask's number.
    pub(crate) masks: u32,
    /// Tiling patterns defined so far, by pattern, matrix and initial state.
    pub(crate) patterns: HashMap<pattern::TilingKey, PatternId>,
    /// The next tiling pattern's number.
    pub(crate) next_pattern: u32,
    /// Image descriptions read so far, by image and resources.
    pub(crate) images: image::Images,
    /// The next initial-state serial.
    pub(crate) serial: u64,
}

impl std::fmt::Debug for Session<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("needs", &self.needs)
            .field("masks", &self.masks)
            .finish_non_exhaustive()
    }
}

/// Where the text of a content sequence goes (9.4.4): page text, or paint
/// only (annotation appearances, patterns, soft masks, Type 3 glyphs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Text {
    /// Glyphs and marked content are page text.
    Page,
    /// Glyphs are paint only.
    Paint,
}

/// The state of one content sequence.
#[derive(Debug, Clone)]
pub(crate) struct Scope {
    pub(crate) state: Graphics,
    pub(crate) saved: Vec<Graphics>,
    pub(crate) path: PathBuilder,
    pub(crate) pending_clip: Option<FillRule>,
    /// The visibility before each open marked-content sequence.
    pub(crate) marked: Vec<bool>,
    /// Whether optional content leaves marks visible.
    pub(crate) visible: bool,
    /// The CTM when the sequence started, which anchors patterns (8.7.2).
    pub(crate) initial_ctm: Transform,
    /// The graphics state when the sequence started, which tiling cells
    /// start from (8.7.3.1), and its serial.
    pub(crate) initial: Option<(Arc<Graphics>, u64)>,
    /// Inside an uncoloured tiling cell: colour operators are ignored (8.6.8).
    pub(crate) stencil: bool,
    /// Forms, patterns and glyph procedures being interpreted, outermost
    /// first, for cycle detection.
    pub(crate) forms: Vec<ObjectId>,
    pub(crate) text: Option<TextObject>,
    pub(crate) procedure: Option<Procedure>,
    pub(crate) role: Text,
}

impl Scope {
    fn new(state: Graphics, role: Text) -> Self {
        Self {
            initial_ctm: state.ctm,
            state,
            saved: Vec::new(),
            path: PathBuilder::new(),
            pending_clip: None,
            marked: Vec::new(),
            visible: true,
            initial: None,
            stencil: false,
            forms: Vec::new(),
            text: None,
            procedure: None,
            role,
        }
    }

    /// A scope for content nested in this one: it inherits the graphics
    /// state, visibility, cycle stack, stencil restriction and glyph
    /// procedure, and starts with no path, text object or saved states.
    pub(crate) fn child(&self, role: Text) -> Self {
        let mut child = Self::new(self.state.clone(), role);
        child.initial_ctm = self.initial_ctm;
        child.initial.clone_from(&self.initial);
        child.visible = self.visible;
        child.stencil = self.stencil;
        child.forms.clone_from(&self.forms);
        child.procedure.clone_from(&self.procedure);
        child
    }
}

/// Interprets content into a device.
pub(crate) struct Machine<'a, 'd, D: Device + ?Sized> {
    pub(crate) session: Session<'a>,
    pub(crate) device: &'d mut D,
    /// Implicit groups of the content being defined, innermost last.
    pub(crate) lazy: Vec<Lazy>,
    /// The position of the operator being executed.
    pub(crate) position: Position,
}

impl<D: Device + ?Sized> std::fmt::Debug for Machine<'_, '_, D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Machine")
            .field("session", &self.session)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

/// The inputs of one interpretation.
pub(crate) struct Inputs<'a> {
    pub(crate) document: &'a Document,
    pub(crate) provider: Option<&'a dyn pagura_text::Provider>,
    pub(crate) store: Option<&'a Store>,
    pub(crate) filler: Option<&'a pagura_document::forms::FormFiller<'a>>,
    /// The page's resources, for Type 3 fonts without their own; `None`
    /// when content is interpreted outside its page.
    pub(crate) page_resources: Option<&'a Dictionary>,
}

impl<'a, 'd, D: Device + ?Sized> Machine<'a, 'd, D> {
    /// A machine for one page.
    ///
    /// # Errors
    ///
    /// Errors of the document's optional-content configuration.
    pub(crate) fn new(inputs: &Inputs<'a>, device: &'d mut D) -> Result<Self> {
        let document = inputs.document;
        let limits = document.limits();
        let needs = device.needs();
        Ok(Self {
            session: Session {
                document,
                limits,
                needs,
                filler: inputs.filler,
                visibility: Visibility::new(document)?,
                colors: Colors::new(document, inputs.store),
                fonts: Fonts::new(document, inputs.provider, inputs.store, needs.text),
                tokens: Meter::new(limits.operations, "content operation budget exceeded"),
                page_resources: inputs.page_resources,
                name_work: 0,
                opaque: Arc::new(Composite::default()),
                masks: 0,
                patterns: HashMap::new(),
                next_pattern: 0,
                images: image::Images::default(),
                serial: 0,
            },
            device,
            lazy: Vec::new(),
            position: Position::Unknown,
        })
    }

    pub(crate) fn needs(&self) -> Needs {
        self.session.needs
    }

    pub(crate) fn document(&self) -> &'a Document {
        self.session.document
    }

    /// Interprets `page`: its content and its annotations.
    ///
    /// # Errors
    ///
    /// As [`crate::Interpreter::run`].
    pub(crate) fn page(&mut self, page: &'a Page) -> Result<()> {
        let document = self.document();
        let paint = self.needs().paint;
        let group = if paint {
            crate::resources::group::attributes(
                &mut self.session.colors,
                page.dictionary(),
                page.resources(),
            )?
        } else {
            None
        };
        let mut state = Graphics::default();
        if paint {
            let gray = self.session.colors.named(page.resources(), b"DeviceGray")?;
            state.set_both_inks(&gray);
        }
        let streams = document.content_streams(page)?;
        let mut data = Vec::with_capacity(streams.len());
        for stream in &streams {
            data.push((stream.id(), document.stream_data(stream)?));
        }
        let content = Content::from_streams(data, &self.session.limits)?;
        self.device.begin_page(&PageInfo {
            object: page.id(),
            crop: page.crop_box(),
            rotation: page.rotation(),
            user_unit: page.user_unit(),
            group,
            print_intent: crate::resources::print_intent(document, page),
        })?;
        let mut scope = Scope::new(state, Text::Page);
        // Annotations that report at the page's last operator keep its
        // position, so the page's sequence does not restore it.
        self.sequence(&content, page.resources(), &mut scope)?;
        self.annotations(page)?;
        self.device.end_page()
    }

    /// Interprets one content sequence in `scope` (7.8.2).
    ///
    /// # Errors
    ///
    /// Positioned errors of the operators, and `Syntax` for a sequence that
    /// ends with pending operands, an open section, path, clip or text
    /// object; unmatched `q` is a reported repair.
    pub(crate) fn run(
        &mut self,
        content: &Content<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let outer = self.position;
        let result = self.sequence(content, resources, scope);
        self.position = outer;
        result
    }

    fn sequence(
        &mut self,
        content: &Content<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        self.session.serial += 1;
        scope.initial = Some((Arc::new(scope.state.clone()), self.session.serial));
        let mut reader = Reader::new(content, &self.session.limits);
        while let Some(operation) = reader.next(&mut self.session.tokens)? {
            self.position = content.position(operation.offset);
            self.operation(operation.op, &mut reader, resources, scope)
                .map_err(|error| content.locate(error, operation.offset))?;
        }
        if !reader.is_balanced()
            || !scope.marked.is_empty()
            || !scope.path.is_empty()
            || scope.pending_clip.is_some()
            || scope.text.is_some()
        {
            return Err(content.locate(
                Error::syntax("content sequence ends with unfinished state"),
                content.end(),
            ));
        }
        if !scope.saved.is_empty() {
            let located = content.locate(
                Error::syntax("unclosed saved graphics states"),
                content.end(),
            );
            let Position::Stream { object, .. } = located.position else {
                return Err(located);
            };
            self.document().repair(
                Diagnostic::new(
                    repairs::GRAPHICS_UNCLOSED,
                    "unmatched q states discarded at end of content sequence (7.8.2, 8.4.2)",
                )
                .at(located.position)
                .in_object(object),
            )?;
            scope.saved.clear();
        }
        Ok(())
    }

    /// Executes one operation, applying the restrictions of Type 3 glyph
    /// procedures and uncoloured tiling cells first.
    fn operation(
        &mut self,
        op: Op,
        reader: &mut Reader<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        if scope.glyph_restriction(&op)? || self.stencil_ignores(&op, resources, scope)? {
            if let Op::InlineImage(dictionary) = &op {
                self.skip_inline_image(dictionary, reader, resources)?;
            }
            return Ok(());
        }
        let result = self.execute(op, reader, resources, scope);
        if scope.path.len() > self.session.limits.path_segments {
            return Err(Error::limit("path segment budget exceeded"));
        }
        result
    }

    fn execute(
        &mut self,
        op: Op,
        reader: &mut Reader<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        match op {
            Op::Save => scope.save(&self.session.limits),
            Op::Restore => self.restore(scope),
            Op::Concatenate(m) => {
                let matrix = matrix(m).then(scope.state.ctm);
                if !matrix.is_finite() {
                    return Err(Error::syntax("CTM arithmetic overflow"));
                }
                scope.state.set_ctm(matrix);
                Ok(())
            }
            Op::LineWidth(_)
            | Op::LineCap(_)
            | Op::LineJoin(_)
            | Op::MiterLimit(_)
            | Op::Dash(_)
            | Op::Flatness(_)
            | Op::RenderingIntent(_) => params::apply(&op, &mut scope.state),
            Op::MoveTo(_)
            | Op::LineTo(_)
            | Op::CurveTo(_)
            | Op::CurveToInitial(_)
            | Op::CurveToFinal(_)
            | Op::ClosePath
            | Op::Rectangle(_) => self.construct(&op, scope),
            Op::Paint(painting) => self.paint_operator(painting, scope),
            Op::Clip(rule) => path::clip_operator(rule, scope),
            Op::StrokeColorSpace(_)
            | Op::FillColorSpace(_)
            | Op::StrokeColor(_)
            | Op::FillColor(_)
            | Op::StrokeColorN(_)
            | Op::FillColorN(_)
            | Op::StrokeGray(_)
            | Op::FillGray(_)
            | Op::StrokeRgb(_)
            | Op::FillRgb(_)
            | Op::StrokeCmyk(_)
            | Op::FillCmyk(_) => self.color(op, resources, scope),
            Op::ExtGState(name) => self.graphics_state(&name, resources, scope),
            Op::XObject(name) => self.xobject(&name, resources, scope),
            Op::Shading(name) => self.shading_operator(&name, resources, scope),
            Op::BeginMarkedContent(_) | Op::BeginMarkedContentProperties { .. } => {
                self.begin_marked(op, resources, scope)
            }
            Op::EndMarkedContent => self.end_marked(scope),
            Op::MarkedPoint(_)
            | Op::MarkedPointProperties { .. }
            | Op::BeginCompatibility
            | Op::EndCompatibility => Ok(()),
            Op::InlineImage(dictionary) => self.inline_image(&dictionary, reader, resources, scope),
            text => self.text_operator(text, resources, scope),
        }
    }

    /// Interprets one stored annotation appearance as page text, fitted
    /// into `rect`, without the page's content or other annotations.
    ///
    /// # Errors
    ///
    /// As [`Self::page`].
    pub(crate) fn appearance_text(
        &mut self,
        appearance: ObjectId,
        rect: pagura_core::Rect,
        resources: &Dictionary,
    ) -> Result<()> {
        let object = self.document().object(appearance)?;
        self.appearance(form::FormBody::Stored(object), rect, resources, Text::Page)
    }

    /// Reports a repair at the current operator.
    pub(crate) fn repair(&self, tag: pagura_core::Repair, message: &'static str) -> Result<()> {
        let mut diagnostic = Diagnostic::new(tag, message).at(self.position);
        if let Position::Stream { object, .. } = self.position {
            diagnostic = diagnostic.in_object(object);
        }
        self.document().repair(diagnostic)
    }

    /// The next soft mask number.
    pub(crate) fn next_mask(&mut self) -> Result<MaskId> {
        let id = MaskId(self.session.masks);
        self.session.masks = self
            .session
            .masks
            .checked_add(1)
            .ok_or(Error::limit("soft mask count exceeded"))?;
        Ok(id)
    }
}

impl Scope {
    fn save(&mut self, limits: &Limits) -> Result<()> {
        if self.saved.len() >= limits.depth {
            return Err(Error::limit("graphics state nesting exceeded"));
        }
        self.saved.push(self.state.clone());
        Ok(())
    }
}

/// A matrix operand `[a b c d e f]`.
pub(crate) fn matrix(m: [f64; 6]) -> Transform {
    Transform::new(m[0], m[1], m[2], m[3], m[4], m[5])
}

/// Bounds a nesting counter.
pub(crate) fn nested(current: usize, maximum: usize) -> Result<usize> {
    current
        .checked_add(1)
        .filter(|&n| n <= maximum)
        .ok_or(Error::limit("content nesting budget exceeded"))
}
