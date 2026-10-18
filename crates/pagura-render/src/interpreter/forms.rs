//! Isolated form interpretation contexts (8.10).

use super::{
    Arc, Compiler, Dictionary, DisplayList, Error, FillRule, ObjectId, PathBuilder, Rect, Result,
    Transform, Value, features, matrix, resource, unsupported,
};

/// A source program or caller-owned appearance data, with document-relative resources.
#[derive(Clone, Copy)]
pub(super) enum FormBody<'a> {
    Stored(&'a pagura_document::IndirectObject),
    Generated {
        dictionary: &'a Dictionary,
        bytes: &'a [u8],
    },
}
impl<'a> FormBody<'a> {
    pub(super) fn id(self) -> Option<ObjectId> {
        match self {
            Self::Stored(object) => Some(object.id()),
            Self::Generated { .. } => None,
        }
    }
    pub(super) fn dictionary(self) -> Result<&'a Dictionary> {
        match self {
            Self::Stored(object) => object
                .dictionary()
                .filter(|_| object.is_stream())
                .ok_or(Error::syntax("XObject is not a stream")),
            Self::Generated { dictionary, .. } => Ok(dictionary),
        }
    }
}

impl Compiler<'_> {
    pub(super) fn form(&mut self, v: &[Value], resources: &Dictionary) -> Result<()> {
        let name = v
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("XObject name expected"))?;
        let value = resource(self.document, resources, b"XObject", name.as_bytes())?;
        let id = value
            .as_reference()
            .ok_or(Error::syntax("XObject must be a stream reference"))?;
        self.form_object(id, resources).map_err(|e| e.in_object(id))
    }

    pub(super) fn form_object(&mut self, id: ObjectId, resources: &Dictionary) -> Result<()> {
        let object = self.document.object(id)?;
        self.form_body(FormBody::Stored(object), resources)
    }

    pub(super) fn form_body(&mut self, body: FormBody<'_>, resources: &Dictionary) -> Result<()> {
        if body.id().is_some_and(|id| self.forms.contains(&id)) {
            return Err(Error::cycle("recursive form XObject"));
        }
        if self.forms.len() >= self.document.limits().depth {
            return Err(Error::limit("form nesting budget exceeded"));
        }
        let dictionary = body.dictionary()?;
        match dictionary
            .get_name(b"Subtype")
            .map(pagura_core::Name::as_bytes)
        {
            Some(b"Form") => {}
            Some(b"Image") => return Err(Error::syntax("XObject is not a form")),
            _ => return Err(unsupported(features::XOBJECT_SUBTYPE)),
        }
        // Table 93 (8.10.2): skip an invisible form in its entirety.
        let hidden = self.form_hidden(dictionary)?;
        if hidden && self.extraction.is_none() {
            return Ok(());
        }
        if let Some(kind) = dictionary.get(b"FormType")
            && self.document.resolve(kind)?.as_integer() != Some(1)
        {
            return Err(Error::syntax("invalid FormType"));
        }
        let (bbox, matrix) = self.form_geometry(dictionary)?;
        let mut child = self.form_context(body.id(), matrix, bbox)?;
        self.inherit_extraction(&mut child, hidden);
        let bounds = child.state.clip.as_ref().map(|c| Arc::clone(&c.path));
        let resources = dictionary
            .get(b"Resources")
            .map(|v| self.document.resolve_dictionary(v))
            .transpose()?
            .unwrap_or(resources);
        let context = crate::icc::ColorContext::new(self.document, &self.compilation.colors);
        let attributes = crate::transparency::Attributes::parse(&context, dictionary, resources)?;
        if attributes.is_some() {
            child.state.composite = crate::transparency::Composite {
                alpha: 1.0,
                blend: crate::blend::BlendMode::Normal,
                mask: None,
                ..child.state.composite.clone()
            };
            child.state.stroke_alpha = 1.0;
        }
        match body {
            FormBody::Stored(object) => child.extract_form_sequence(object, resources)?,
            FormBody::Generated { bytes, .. } => {
                child.position = pagura_core::Position::Unknown;
                child.sequence_bytes(bytes, &[], resources)?;
            }
        }
        self.operations = child.operations;
        self.marks = child.marks;
        if self
            .list
            .commands
            .len()
            .checked_add(child.list.commands.len())
            .is_none_or(|n| n > self.document.limits().display_commands)
        {
            return Err(Error::limit("display list budget exceeded"));
        }
        if let Some(attributes) = attributes {
            self.marks = self
                .marks
                .checked_add(1)
                .filter(|&n| n <= self.document.limits().display_commands)
                .ok_or(Error::limit("display list budget exceeded"))?;
            self.list
                .commands
                .push(crate::display::Command::Group(crate::transparency::Group {
                    position: self.position,
                    bounds,
                    attributes,
                    commands: child.list.commands,
                    composite: self.state.composite.clone(),
                }));
        } else {
            self.list.commands.extend(child.list.commands);
        }
        Ok(())
    }
    pub(super) fn form_geometry(&self, dictionary: &Dictionary) -> Result<(Rect, Transform)> {
        let bbox = dictionary
            .get(b"BBox")
            .map(|v| self.document.resolve(v))
            .transpose()?
            .and_then(Value::as_array)
            .and_then(Rect::from_array)
            .ok_or(Error::syntax("form has invalid BBox"))?;
        let matrix = dictionary
            .get(b"Matrix")
            .map(|v| {
                let values = self
                    .document
                    .resolve(v)?
                    .as_array()
                    .ok_or(Error::syntax("form Matrix must be an array"))?;
                matrix(values)
            })
            .transpose()?
            .unwrap_or(Transform::IDENTITY);
        Ok((bbox, matrix))
    }
    pub(super) fn form_hidden(&self, dictionary: &Dictionary) -> Result<bool> {
        Ok(dictionary
            .get(b"OC")
            .map(|oc| self.visibility.visible(self.document, oc))
            .transpose()?
            .is_some_and(|visible| !visible))
    }
    pub(super) fn form_context(
        &self,
        id: Option<ObjectId>,
        matrix: Transform,
        bbox: Rect,
    ) -> Result<Self> {
        let mut child = self.detached();
        if let Some(id) = id {
            child.forms.push(id);
        }
        child.state.ctm = matrix.then(child.state.ctm);
        if !child.state.ctm.is_finite() {
            return Err(Error::syntax("form matrix arithmetic overflow"));
        }
        child.initial_ctm = child.state.ctm;
        let mut clip = PathBuilder::new();
        clip.rect(bbox.x0(), bbox.y0(), bbox.width(), bbox.height());
        child.add_clip(
            Arc::new(clip.finish().transformed(child.state.ctm)),
            FillRule::NonZero,
        )?;
        Ok(child)
    }
    pub(super) fn detached(&self) -> Self {
        Self {
            document: self.document,
            compilation: std::rc::Rc::clone(&self.compilation),
            list: DisplayList {
                commands: Vec::new(),
                crop: self.list.crop,
                rotation: self.list.rotation,
                user_unit: self.list.user_unit,
                limits: self.list.limits,
                group: self.list.group,
                print_intent: self.list.print_intent,
                separations: std::sync::OnceLock::new(),
            },
            state: self.state.clone(),
            saved: Vec::new(),
            path: PathBuilder::new(),
            pending_clip: None,
            operands: Vec::new(),
            compatibility: 0,
            marked: Vec::new(),
            visible: self.visible,
            visibility: Arc::clone(&self.visibility),
            operations: self.operations,
            fill_work: std::rc::Rc::clone(&self.fill_work),
            marks: self.marks,
            initial_ctm: self.initial_ctm,
            initial_state: self.initial_state.clone(),
            stencil_pattern: self.stencil_pattern,
            position: self.position,
            forms: self.forms.clone(),
            extraction: None,
        }
    }
}

#[cfg(test)]
mod tests;
