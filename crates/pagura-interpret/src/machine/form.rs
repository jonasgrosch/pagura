//! XObjects (8.8): form content interpreted in place, groups around forms
//! with a `Group` (8.10, 11.6.6), and the dispatch to images.

use super::emit::GroupStart;
use super::path::add_clip;
use super::{Machine, Scope, matrix};
use crate::device::{Device, Marked};
use crate::features;
use crate::model::GroupCause;
use crate::resources::group;
use pagura_content::Content;
use pagura_core::{Dictionary, Error, ObjectId, Rect, Result, Value};
use pagura_document::Document;
use pagura_raster::{FillRule, Path, Transform};
use std::borrow::Cow;
use std::sync::Arc;

/// A form's program: a stream of the document, or appearance data held in
/// memory, whose resources still refer to the document.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FormBody<'a> {
    Stored(&'a pagura_document::IndirectObject),
    Generated {
        dictionary: &'a Dictionary,
        bytes: &'a [u8],
    },
}

impl<'a> FormBody<'a> {
    pub(crate) fn id(self) -> Option<ObjectId> {
        match self {
            Self::Stored(object) => Some(object.id()),
            Self::Generated { .. } => None,
        }
    }

    pub(crate) fn dictionary(self) -> Result<&'a Dictionary> {
        match self {
            Self::Stored(object) => object
                .dictionary()
                .filter(|_| object.is_stream())
                .ok_or(Error::syntax("XObject is not a stream")),
            Self::Generated { dictionary, .. } => Ok(dictionary),
        }
    }
}

/// What happens to a form that optional content hides (8.11.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HiddenForm {
    /// It is skipped in its entirety (Table 93).
    Skip,
    /// It is interpreted, its marks flagged hidden.
    Interpret,
}

/// A form prepared for interpretation: its scope, content and resources.
pub(crate) struct PreparedForm<'a> {
    pub(crate) scope: Scope,
    pub(crate) content: Arc<Content<'a>>,
    pub(crate) resources: &'a Dictionary,
    pub(crate) dictionary: &'a Dictionary,
    /// The clip of the form's bounding box in default user space.
    pub(crate) bounds: Option<Arc<Path>>,
}

/// The resource `name` of category `kind`.
///
/// # Errors
///
/// `Syntax` for an undefined resource.
pub(crate) fn resource<'a>(
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

/// A form's `BBox` and `Matrix` (Table 93).
///
/// # Errors
///
/// `Syntax` for a missing or malformed `BBox` or `Matrix`.
pub(crate) fn geometry(document: &Document, dictionary: &Dictionary) -> Result<(Rect, Transform)> {
    let bbox = dictionary
        .get(b"BBox")
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_array)
        .and_then(Rect::from_array)
        .ok_or(Error::syntax("form has invalid BBox"))?;
    let matrix = dictionary
        .get(b"Matrix")
        .map(|v| {
            let values = document
                .resolve(v)?
                .as_array()
                .ok_or(Error::syntax("form Matrix must be an array"))?;
            if values.len() != 6 {
                return Err(Error::syntax("matrix must have six numbers"));
            }
            let mut m = [0.0; 6];
            for (out, value) in m.iter_mut().zip(values) {
                *out = value
                    .as_number()
                    .ok_or(Error::syntax("numeric operand expected"))?;
            }
            Ok(matrix(m))
        })
        .transpose()?
        .unwrap_or(Transform::IDENTITY);
    Ok((bbox, matrix))
}

impl<'a, D: Device + ?Sized> Machine<'a, '_, D> {
    /// `Do`: a form or an image.
    pub(super) fn xobject(
        &mut self,
        name: &pagura_core::Name,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let document = self.document();
        let value = resource(document, resources, b"XObject", name.as_bytes())?;
        let id = value
            .as_reference()
            .ok_or(Error::syntax("XObject must be a stream reference"))?;
        let object = document.object(id)?;
        let dictionary = object
            .dictionary()
            .ok_or(Error::syntax("XObject has no dictionary"))?;
        if dictionary.is_name(b"Subtype", b"Image") {
            return self.image_xobject(id, dictionary, resources, scope);
        }
        self.form(FormBody::Stored(object), resources, scope)
            .map_err(|e| e.in_object(id))
    }

    /// Whether optional content hides the form (Table 93).
    pub(crate) fn form_hidden(&self, dictionary: &Dictionary) -> Result<bool> {
        Ok(dictionary
            .get(b"OC")
            .map(|oc| self.session.visibility.visible(self.document(), oc))
            .transpose()?
            .is_some_and(|visible| !visible))
    }

    /// Interprets a form in place (8.10): its content with its own resources
    /// in a nested scope, clipped to its bounding box, inside a group when it
    /// has a `Group`.
    pub(crate) fn form(
        &mut self,
        body: FormBody<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        // Devices that need structure receive hidden marks, flagged.
        let hidden = if self.needs().structure {
            HiddenForm::Interpret
        } else {
            HiddenForm::Skip
        };
        let Some(form) = self.prepare_form(body, resources, scope, hidden)? else {
            return Ok(());
        };
        let PreparedForm {
            scope: mut child,
            content,
            resources,
            dictionary,
            bounds,
        } = form;
        let attributes = if self.needs().paint {
            group::attributes(&mut self.session.colors, dictionary, resources)?
        } else {
            None
        };
        let grouped = attributes.is_some() && child.visible;
        if let Some(attributes) = attributes {
            let composite = scope.state.fill_composite();
            child.state.start_group();
            if grouped {
                self.begin_group(
                    &GroupStart {
                        cause: GroupCause::Document,
                        attributes,
                        bbox: bounds.as_ref(),
                        view: None,
                        composite: &composite,
                    },
                    scope,
                )?;
            }
        }
        let occurrence = self.structure(scope).then(|| body.id()).flatten();
        if let Some(object) = occurrence {
            self.device.marked(&Marked::ObjectBegin {
                object,
                hidden: !child.visible,
            })?;
        }
        self.run(&content, resources, &mut child)?;
        if occurrence.is_some() {
            self.device.marked(&Marked::ObjectEnd)?;
        }
        if grouped {
            self.device.end_group()?;
        }
        Ok(())
    }

    /// Checks a form and builds its scope and content, or `None` for a form
    /// optional content hides when `hidden` skips it.
    pub(crate) fn prepare_form<'f>(
        &mut self,
        body: FormBody<'f>,
        resources: &'f Dictionary,
        scope: &Scope,
        hidden: HiddenForm,
    ) -> Result<Option<PreparedForm<'f>>>
    where
        'a: 'f,
    {
        let document = self.document();
        if body.id().is_some_and(|id| scope.forms.contains(&id)) {
            return Err(Error::cycle("recursive form XObject"));
        }
        if scope.forms.len() >= self.session.limits.depth {
            return Err(Error::limit("form nesting budget exceeded"));
        }
        let dictionary = body.dictionary()?;
        match dictionary
            .get_name(b"Subtype")
            .map(pagura_core::Name::as_bytes)
        {
            Some(b"Form") => {}
            Some(b"Image") => return Err(Error::syntax("XObject is not a form")),
            _ => {
                return Err(Error::unsupported(
                    features::XOBJECT_SUBTYPE,
                    "rendering feature is not implemented",
                ));
            }
        }
        // Table 93 (8.10.2): skip an invisible form in its entirety.
        let invisible = self.form_hidden(dictionary)?;
        if invisible && hidden == HiddenForm::Skip {
            return Ok(None);
        }
        if let Some(kind) = dictionary.get(b"FormType")
            && document.resolve(kind)?.as_integer() != Some(1)
        {
            return Err(Error::syntax("invalid FormType"));
        }
        let (bbox, matrix) = geometry(document, dictionary)?;
        let mut child = scope.child(scope.role);
        if let Some(id) = body.id() {
            child.forms.push(id);
        }
        let ctm = matrix.then(child.state.ctm);
        if !ctm.is_finite() {
            return Err(Error::syntax("form matrix arithmetic overflow"));
        }
        child.state.set_ctm(ctm);
        child.initial_ctm = ctm;
        let bounds = Arc::new(Path::rect(bbox).transformed(ctm));
        add_clip(
            &mut child,
            Arc::clone(&bounds),
            FillRule::NonZero,
            self.session.limits.depth,
        )?;
        child.visible &= !invisible;
        let resources = dictionary
            .get(b"Resources")
            .map(|v| document.resolve_dictionary(v))
            .transpose()?
            .unwrap_or(resources);
        let content = match body {
            FormBody::Stored(object) => self.stream_content(object)?,
            FormBody::Generated { bytes, .. } => {
                Arc::new(Content::from_bytes(Cow::Borrowed(bytes)))
            }
        };
        Ok(Some(PreparedForm {
            scope: child,
            content,
            resources,
            dictionary,
            bounds: Some(bounds),
        }))
    }
}
