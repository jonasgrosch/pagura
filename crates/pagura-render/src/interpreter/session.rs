//! One page compilation's shared state, and the child contexts that forms,
//! soft masks, tiling cells and Type 3 glyphs interpret in.
//!
//! Graphics state is saved and restored with `q`/`Q` and copied into child
//! contexts; nothing in it may be compilation-wide. Budgets, caches and
//! extracted text live in the [`Session`] that every context borrows, so an
//! annotation appearance or a pattern cell cannot start from a fresh budget.

use super::text::{self, cache::FontCache, extract, type3};
use super::{Compiler, FillRule, Graphics, PathBuilder, Result};
use crate::color::Color;
use crate::optional::Visibility;
use crate::pattern::{PaintType, TilingDefinition};
use pagura_core::{Error, Meter, ObjectId, Rect};
use pagura_document::{Document, Page};
use pagura_raster::Transform;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub(super) struct Session<'a> {
    pub(super) document: &'a Document,
    pub(super) page: &'a Page,
    pub(super) provider: Option<&'a dyn pagura_text::Provider>,
    /// The document's font cache, shared across pages by a `RenderContext`.
    pub(super) fonts: &'a Mutex<FontCache>,
    /// In-memory form values whose widgets get generated appearances.
    pub(super) filler: Option<&'a pagura_document::forms::FormFiller<'a>>,
    profiles: crate::icc::Cache,
    pub(super) visibility: Visibility,
    /// Fonts, outlines and images this compilation retains.
    pub(super) paint: RefCell<text::cache::Cache>,
    /// Positioned text, when the compilation also extracts text.
    pub(super) text: Option<RefCell<extract::Output>>,
    /// Content tokens, inline-image header entries and shown glyphs.
    operations: RefCell<Meter>,
    /// Painted marks and groups in the display list.
    marks: RefCell<Meter>,
    /// Bytes this compilation holds (ADR 0016 `render_bytes`): the paint and
    /// colour caches, mesh data and content buffers while they are parsed.
    retained: RefCell<Meter>,
    /// Raster work certifying the regions of filled paths and clips
    /// (10.7.4), charged once per path for the whole compilation.
    pub(super) fill_work: Cell<usize>,
}

impl<'a> Session<'a> {
    pub(super) fn new(
        document: &'a Document,
        page: &'a Page,
        provider: Option<&'a dyn pagura_text::Provider>,
        fonts: &'a Mutex<FontCache>,
        filler: Option<&'a pagura_document::forms::FormFiller<'a>>,
        text: Option<extract::Output>,
    ) -> Result<Self> {
        let limits = document.limits();
        Ok(Self {
            document,
            page,
            provider,
            fonts,
            filler,
            profiles: crate::icc::Cache::default(),
            visibility: Visibility::new(document)?,
            paint: RefCell::default(),
            text: text.map(RefCell::new),
            operations: RefCell::new(Meter::new(
                limits.operations,
                "content operation budget exceeded",
            )),
            marks: RefCell::new(Meter::new(
                limits.display_commands,
                "display list budget exceeded",
            )),
            retained: RefCell::new(Meter::new(
                limits.render_bytes,
                "page compilation byte budget exceeded",
            )),
            fill_work: Cell::new(0),
        })
    }

    /// Charges interpretation work against the page's `operations` budget.
    pub(super) fn charge_operations(&self, count: usize) -> Result<()> {
        self.operations.borrow_mut().charge(count)
    }

    /// Charges painted marks against the page's `display_commands` budget.
    pub(super) fn charge_marks(&self, count: usize) -> Result<()> {
        self.marks.borrow_mut().charge(count)
    }

    /// Charges bytes the compilation keeps until it ends.
    pub(super) fn retain(&self, bytes: usize) -> Result<()> {
        self.retained.borrow_mut().charge(bytes)
    }

    /// Charges bytes held until the returned guard is dropped.
    pub(super) fn hold(&self, bytes: usize) -> Result<Held<'_>> {
        self.retain(bytes)?;
        Ok(Held {
            meter: &self.retained,
            bytes,
        })
    }

    /// Colour-space context whose profile cache charges this session.
    pub(super) fn colors(&self) -> crate::icc::ColorContext<'_> {
        crate::icc::ColorContext::new(self.document, &self.profiles, &self.retained)
    }

    #[cfg(test)]
    pub(super) fn retained(&self) -> usize {
        self.retained.borrow().used()
    }

    #[cfg(test)]
    pub(super) fn marks(&self) -> usize {
        self.marks.borrow().used()
    }

    #[cfg(test)]
    pub(super) fn operations(&self) -> usize {
        self.operations.borrow().used()
    }
}

/// Bytes charged to a session for as long as the guard lives.
pub(super) struct Held<'m> {
    meter: &'m RefCell<Meter>,
    bytes: usize,
}

impl Held<'_> {
    /// Charges `bytes` more, released with the rest.
    pub(super) fn add(&mut self, bytes: usize) -> Result<()> {
        self.meter.borrow_mut().charge(bytes)?;
        self.bytes += bytes;
        Ok(())
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        self.meter.borrow_mut().refund(self.bytes);
    }
}

/// A content stream interpreted inside another one.
#[derive(Clone, Copy)]
pub(super) enum Child<'k> {
    /// A form XObject (8.10.1) in the parent's graphics state, clipped to its
    /// BBox. A transparency group (11.6.6) also starts with opaque Normal
    /// compositing and no soft mask.
    /// `id` is `None` for a generated appearance stream (12.7.4.3).
    Form {
        id: Option<ObjectId>,
        matrix: Transform,
        bbox: Rect,
        group: bool,
    },
    /// The source of a soft mask (11.6.5.1): the parent's state without clip,
    /// opacity, blend mode or mask. Its group form is a child of this one.
    SoftMask,
    /// A tiling cell (8.7.3.1) in the initial graphics state of the stream
    /// that selected the pattern, in pattern space. An uncoloured cell
    /// paints with `color` and ignores its own colours (8.6.8).
    Pattern {
        tiling: &'k TilingDefinition,
        color: Option<&'k Color>,
    },
    /// A Type 3 glyph procedure (9.6.4) at the glyph's transform.
    Glyph {
        id: ObjectId,
        transform: Transform,
        expected: type3::Advance,
    },
}

impl<'s> Compiler<'s> {
    pub(super) fn new(session: &'s Session<'s>) -> Self {
        Self {
            session,
            document: session.document,
            commands: Vec::new(),
            state: Graphics::default(),
            saved: Vec::new(),
            path: PathBuilder::new(),
            pending_clip: None,
            operands: Vec::new(),
            compatibility: 0,
            marked: Vec::new(),
            visible: true,
            initial_ctm: Transform::IDENTITY,
            initial_state: Graphics::default(),
            stencil_pattern: false,
            position: pagura_core::Position::Unknown,
            forms: Vec::new(),
            text_object: None,
            text_run: None,
            procedure: None,
            extraction: session
                .text
                .as_ref()
                .map(|_| extract::Scope::new(session.page.id())),
            patterns: BTreeMap::new(),
        }
    }

    /// A context for content interpreted inside this one. It shares the
    /// session, inherits graphics state, visibility, the form stack and the
    /// uncoloured-glyph restriction, and starts with no path, operands,
    /// saved states, marked content or text object.
    pub(super) fn child(&self, kind: Child<'_>) -> Result<Self> {
        let mut child = Self {
            session: self.session,
            document: self.document,
            commands: Vec::new(),
            state: self.state.clone(),
            saved: Vec::new(),
            path: PathBuilder::new(),
            pending_clip: None,
            operands: Vec::new(),
            compatibility: 0,
            marked: Vec::new(),
            visible: self.visible,
            initial_ctm: self.initial_ctm,
            initial_state: self.initial_state.clone(),
            stencil_pattern: self.stencil_pattern,
            position: self.position,
            forms: self.forms.clone(),
            text_object: None,
            text_run: None,
            procedure: self.procedure.clone(),
            extraction: None,
            patterns: BTreeMap::new(),
        };
        match kind {
            Child::Form {
                id,
                matrix,
                bbox,
                group,
            } => {
                if let Some(id) = id {
                    child.enter(id, "recursive form XObject")?;
                }
                child.state.ctm = matrix.then(child.state.ctm);
                if !child.state.ctm.is_finite() {
                    return Err(Error::syntax("form matrix arithmetic overflow"));
                }
                child.initial_ctm = child.state.ctm;
                if group {
                    child.state.isolate();
                }
                let clip = crate::device::rectangle(bbox).transformed(child.state.ctm);
                child.add_clip(Arc::new(clip), FillRule::NonZero)?;
                child.extraction =
                    id.and_then(|id| self.extraction.as_ref().map(|scope| scope.within(id)));
            }
            Child::SoftMask => {
                child.state.clip = None;
                child.state.clip_depth = 0;
                child.state.isolate();
            }
            Child::Pattern { tiling, color } => {
                child.enter(tiling.id, "recursive tiling pattern")?;
                child.state = self.initial_state.pattern_cell();
                child.procedure = None;
                child.initial_ctm = Transform::IDENTITY;
                child.initial_state = child.state.clone();
                child.stencil_pattern = tiling.paint == PaintType::Uncolored;
                if let Some(color) = color {
                    child.state.fill.color = Color::clone(color);
                    child.state.stroke.color = Color::clone(color);
                }
                child.add_clip(
                    Arc::new(crate::device::rectangle(tiling.bbox)),
                    FillRule::NonZero,
                )?;
            }
            Child::Glyph {
                id,
                transform,
                expected,
            } => {
                child.enter(id, "recursive Type3 glyph")?;
                child.state.ctm = transform;
                child.initial_ctm = transform;
                child.procedure = Some(type3::Procedure::new(self.uncolored_glyph(), expected));
            }
        }
        Ok(child)
    }

    /// Pushes a form, pattern or glyph onto the stack that detects cycles
    /// and bounds nesting.
    fn enter(&mut self, id: ObjectId, cycle: &'static str) -> Result<()> {
        if self.forms.contains(&id) {
            return Err(Error::cycle(cycle));
        }
        if self.forms.len() >= self.document.limits().depth {
            return Err(Error::limit("form, pattern or glyph nesting exceeded"));
        }
        self.forms.push(id);
        Ok(())
    }
}
