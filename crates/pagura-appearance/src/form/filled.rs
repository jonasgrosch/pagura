//! Source-widget identity and lazy filled appearance selection (12.7.4.3).

use super::{UpdatePolicy, original, widget_appearance};
use crate::fonts::FontSource;
use crate::information;
use crate::stream::Appearance;
use pagura_core::{Diagnostic, Dictionary, Error, ErrorKind, Rect, Result};
use pagura_document::forms::{FieldKind, FormFiller};
use std::collections::BTreeMap;

/// The widgets of a form session, found by the identity of their annotation
/// dictionaries, for drawing a page with in-memory form values.
///
/// Changed widgets use generated text and choice programs or the selected
/// button state; NeedAppearances also regenerates unchanged text and choice
/// fields. Missing text/choice appearances use their source values even without
/// edits. An untouched field with a stored AP keeps it when generation is
/// unsupported, reporting [`information::APPEARANCE_KEPT`] in both
/// recovery modes; edited fields and other errors fail.
#[derive(Debug)]
pub struct FilledWidgets<'f, 'd> {
    filler: &'f FormFiller<'d>,
    widgets: BTreeMap<usize, usize>,
}

impl<'f, 'd> FilledWidgets<'f, 'd> {
    /// Indexes the widgets of `filler` by dictionary identity.
    ///
    /// # Errors
    /// `Limit` when the index exceeds the document byte budget.
    pub fn new(filler: &'f FormFiller<'d>) -> Result<Self> {
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

    /// The form session.
    pub fn filler(&self) -> &'f FormFiller<'d> {
        self.filler
    }

    /// The appearance to draw for the annotation `annotation` with
    /// rectangle `rect`, or `None` to draw it as the document stores it.
    ///
    /// `annotation` must be the dictionary the filler's document resolves
    /// for the annotation: identity, not content, selects the widget, so a
    /// value never applies to an unrelated annotation with the same field
    /// name. An untouched widget keeps its stored appearance unless
    /// NeedAppearances requests regeneration; missing Tx/Ch appearances are
    /// generated from source values. When the widget selects a stored stream,
    /// `culled` receives that stream's dictionary and returns whether the
    /// stream would paint nothing (hidden by optional content, or an empty
    /// fitted placement, 12.5.5); a culled stream is returned as it is,
    /// without generating text.
    ///
    /// # Errors
    /// The errors of `culled`, and those of [`super::prepare`] placed at the
    /// widget, except `Unsupported` for an untouched field with a stored
    /// appearance, which keeps that appearance.
    pub fn appearance(
        &self,
        annotation: &Dictionary,
        rect: Rect,
        fonts: &dyn FontSource,
        culled: impl FnOnce(&Dictionary) -> Result<bool>,
    ) -> Result<Option<Appearance>> {
        let Some(&index) = self.widgets.get(&std::ptr::from_ref(annotation).addr()) else {
            return Ok(None);
        };
        let form = self.filler.form();
        let widget = super::widget(self.filler, index)?;
        let field = super::field(self.filler, widget)?;
        let variable = matches!(
            field.kind,
            FieldKind::Text { .. } | FieldKind::Choice { .. }
        );
        let regenerate =
            self.filler.get(widget.field).is_some() || form.need_appearances && variable;
        let original = original(self.filler, index)?;
        if !regenerate
            && rect.width() != 0.0
            && rect.height() != 0.0
            && (!variable || original.is_some())
        {
            return Ok(None);
        }
        let document = self.filler.document();
        if let Some(Appearance::Existing(id)) = original.as_ref() {
            let stream = document
                .object(*id)?
                .dictionary()
                .ok_or(Error::syntax("appearance has no dictionary"))?;
            if culled(stream)? {
                return Ok(original);
            }
        }
        match widget_appearance(self.filler, index, fonts, UpdatePolicy::Specification) {
            Err(error)
                if self.filler.get(widget.field).is_none()
                    && matches!(error.kind, ErrorKind::Unsupported(_))
                    && matches!(original, Some(Appearance::Existing(_))) =>
            {
                // Information, valid in Strict mode too: the source data is valid.
                let diagnostic = Diagnostic::information(
                    information::APPEARANCE_KEPT,
                    "stored appearance retained because generation is unsupported",
                )
                .at(widget.position)
                .in_object_opt(widget.object);
                document.report([diagnostic]);
                Ok(original)
            }
            result => result.map(Some),
        }
    }
}
