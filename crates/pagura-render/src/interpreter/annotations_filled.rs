//! Source-widget identity and lazy filled appearance selection (12.7.4.3).
use super::{Dictionary, DisplayList, Error, Page, Result};
use crate::form_appearance::{self, Appearance};
use pagura_document::forms::{FieldKind, FormFiller};
use pagura_text::Provider;
use std::collections::BTreeMap;

/// Compiles a page with the current in-memory form values (12.7.4.3).
///
/// Changed widgets use generated text/choice programs or the selected button
/// state. NeedAppearances also regenerates unchanged text and choice fields.
/// Visibility, placement, clipping and optional content follow normal annotation
/// rendering. Source objects are immutable; appearance data is temporary.
/// An untouched field with a stored AP keeps it when generation is unsupported,
/// reporting `form.appearance-kept` information in both Strict and Repair modes.
/// Edited fields and non-capability errors still fail explicitly.
///
/// # Errors
/// As [`super::compile_page`] and [`form_appearance::prepare_widget`]. A page
/// belonging to another document is rejected. Only visible widgets on this page
/// need appearance generation; no partial display list is returned on failure.
pub fn compile_filled_page(
    filler: &FormFiller<'_>,
    page: &Page,
    provider: Option<&dyn Provider>,
) -> Result<DisplayList> {
    let fonts = std::sync::Mutex::new(super::text::cache::FontCache::new(
        filler.document().limits(),
    ));
    compile_filled_cached(filler, page, provider, &fonts)
}

pub(crate) fn compile_filled_cached(
    filler: &FormFiller<'_>,
    page: &Page,
    provider: Option<&dyn Provider>,
    fonts: &std::sync::Mutex<super::text::cache::FontCache>,
) -> Result<DisplayList> {
    let document = filler.document();
    let owned = document.page(page.index())?;
    if owned != page || !std::ptr::eq(owned.resources(), page.resources()) {
        return Err(Error::invalid_argument(
            "page does not belong to form document",
        ));
    }
    let mut compiler = super::Compiler::new(document, page)?;
    compiler.compilation = std::rc::Rc::new(super::Compilation {
        colors: crate::icc::Cache::default(),
        provider,
        fonts: Some(fonts),
        filler: Some(filler),
    });
    compiler.page(page).map_err(|e| e.in_object(page.id()))?;
    Ok(compiler.list)
}

pub(super) struct FilledAnnotations<'f, 'd> {
    filler: &'f FormFiller<'d>,
    widgets: BTreeMap<usize, usize>,
}

impl<'f, 'd> FilledAnnotations<'f, 'd> {
    pub(super) fn new(filler: &'f FormFiller<'d>) -> Result<Self> {
        let widgets = &filler.form().widgets;
        widgets
            .len()
            .checked_mul(64 + size_of::<(usize, usize)>())
            .filter(|&n| n <= filler.document().limits().document_bytes)
            .ok_or(Error::limit("filled widget index budget exceeded"))?;
        Ok(Self {
            filler,
            widgets: widgets
                .iter()
                .enumerate()
                .map(|(i, widget)| (std::ptr::from_ref(widget.dictionary).addr(), i))
                .collect(),
        })
    }

    pub(super) fn prepare(
        &self,
        dictionary: &Dictionary,
        compiler: &super::Compiler<'_>,
        rect: super::Rect,
    ) -> Result<Option<Appearance>> {
        // Dictionaries are borrowed from the immutable document. Identity avoids
        // applying a value to an unrelated annotation with the same field name.
        let Some(&index) = self.widgets.get(&std::ptr::from_ref(dictionary).addr()) else {
            return Ok(None);
        };
        let widget = &self.filler.form().widgets[index];
        let field = &self.filler.form().fields[widget.field];
        let regenerate = self.filler.get(widget.field).is_some()
            || self.filler.form().need_appearances
                && matches!(
                    field.kind,
                    FieldKind::Text { .. } | FieldKind::Choice { .. }
                );
        if !regenerate
            && matches!(
                field.kind,
                FieldKind::Text { .. } | FieldKind::Choice { .. } | FieldKind::Button { .. }
            )
            && rect.width() != 0.0
            && rect.height() != 0.0
        {
            return Ok(None);
        }
        let original = form_appearance::original(self.filler, index)?;
        if let Some(Appearance::Existing(id)) = original.as_ref() {
            let form = compiler
                .document
                .object(*id)?
                .dictionary()
                .ok_or(Error::syntax("appearance has no dictionary"))?;
            let (bbox, matrix) = compiler.form_geometry(form)?;
            if compiler.form_hidden(form)?
                || super::annotations::placement(bbox, matrix, rect)?.is_none()
            {
                return Ok(original);
            }
        }
        match form_appearance::prepare_widget_cached(
            self.filler,
            index,
            compiler.compilation.provider,
            compiler.compilation.fonts,
        ) {
            Err(error)
                if self.filler.get(widget.field).is_none()
                    && matches!(error.kind, pagura_core::ErrorKind::Unsupported(_))
                    && matches!(original, Some(Appearance::Existing(_))) =>
            {
                // Information, valid in Strict mode too: the source data is valid.
                let mut diagnostic = pagura_core::Diagnostic::information(
                    super::tags::APPEARANCE_KEPT,
                    "stored appearance retained because generation is unsupported",
                )
                .at(widget.position);
                diagnostic.object = widget.object;
                compiler.document.report([diagnostic]);
                Ok(original)
            }
            result => result.map(Some),
        }
    }
}
