//! Marked content (14.6) and the optional content it selects (8.11.3.2).
//! Devices that need structure receive the sequences of page content and
//! forms; for devices that need text, the properties text extraction reads
//! are parsed here, with malformed values reported and read as absent.

use super::form::resource;
use super::{Machine, Scope, Text};
use crate::device::{Device, Marked, MarkedText};
use pagura_content::{Op, Properties};
use pagura_core::{
    Diagnostic, Diagnostics, Dictionary, Error, ErrorKind, Position, Recovery, Result, Value,
};

impl<'a, D: Device + ?Sized> Machine<'a, '_, D> {
    /// `BMC` and `BDC`.
    pub(super) fn begin_marked(
        &mut self,
        op: Op<'_>,
        resources: &Dictionary,
        scope: &mut Scope,
    ) -> Result<()> {
        let document = self.document();
        if scope.marked.len() >= self.session.limits.depth {
            return Err(Error::limit("marked content nesting exceeded"));
        }
        let (tag, properties) = match op {
            Op::BeginMarkedContent(tag) => (tag, None),
            Op::BeginMarkedContentProperties { tag, properties } => (tag, Some(properties)),
            _ => return Err(Error::syntax("invalid marked content operator")),
        };
        let mut visible = true;
        if tag.as_bytes() == b"OC"
            && let Some(properties) = &properties
        {
            let inline;
            let value = match properties {
                Properties::Name(name) => {
                    resource(document, resources, b"Properties", name.as_bytes())?
                }
                Properties::Inline(dictionary) => {
                    inline = Value::Dictionary(dictionary.as_ref().clone());
                    &inline
                }
            };
            visible = self.session.visibility.marked(document, value)?;
        }
        scope.marked.push(scope.visible);
        scope.visible &= visible;
        if !self.structure(scope) {
            return Ok(());
        }
        let dictionary = match &properties {
            Some(properties) => self.properties(properties, resources)?,
            None => None,
        };
        let text = if self.needs().text {
            Some(self.marked_text(dictionary)?)
        } else {
            None
        };
        self.device.marked(&Marked::Begin {
            tag: &tag,
            properties: dictionary,
            hidden: !scope.visible,
            text: text.as_ref(),
        })
    }

    /// `EMC`.
    pub(super) fn end_marked(&mut self, scope: &mut Scope) -> Result<()> {
        scope.visible = scope
            .marked
            .pop()
            .ok_or(Error::syntax("EMC without marked content"))?;
        if self.structure(scope) {
            self.device.marked(&Marked::End)?;
        }
        Ok(())
    }

    /// Whether the device receives the marked content of `scope`.
    pub(crate) fn structure(&self, scope: &Scope) -> bool {
        self.needs().structure && scope.role == Text::Page
    }

    /// The properties dictionary, or `None` after reporting a malformed one.
    fn properties<'r>(
        &self,
        properties: &'r Properties,
        resources: &'r Dictionary,
    ) -> Result<Option<&'r Dictionary>>
    where
        'a: 'r,
    {
        let document = self.document();
        let result = match properties {
            Properties::Inline(dictionary) => Ok(dictionary.as_ref()),
            Properties::Name(name) => resource(document, resources, b"Properties", name.as_bytes())
                .and_then(|value| document.resolve_dictionary(value)),
        };
        match result {
            Ok(dictionary) => Ok(Some(dictionary)),
            Err(error) => {
                self.ignore_metadata_fault(error)?;
                Ok(None)
            }
        }
    }

    /// `MCID`, `ActualText` and `Alt` (14.7.5.2, 14.9.3–14.9.4); `E` is
    /// checked but not kept.
    fn marked_text(&self, dictionary: Option<&Dictionary>) -> Result<MarkedText> {
        let actual_text = self.property(dictionary, b"ActualText")?;
        let alt = self.property(dictionary, b"Alt")?;
        // Expansion text is speech metadata, not replacement text.
        self.property(dictionary, b"E")?;
        Ok(MarkedText {
            mcid: self.marked_id(dictionary)?,
            actual_text,
            alt,
        })
    }

    fn property(&self, dictionary: Option<&Dictionary>, name: &[u8]) -> Result<Option<String>> {
        let Some(value) = dictionary.and_then(|d| d.get(name)) else {
            return Ok(None);
        };
        let document = self.document();
        let result = (|| {
            let value = document.resolve(value)?;
            if value.is_null() {
                return Ok(None);
            }
            let message = match name {
                b"ActualText" => "non-string marked ActualText ignored (14.9.4)",
                b"Alt" => "non-string marked Alt ignored (14.9.3)",
                _ => "non-string marked E ignored (14.9.5)",
            };
            let bytes = value.as_string().ok_or(Error::syntax(message))?;
            if name == b"E" {
                return Ok(None);
            }
            let mut strict = Diagnostics::new(Recovery::Strict);
            pagura_core::text_string::decode(bytes, &document.limits(), &mut strict).map(Some)
        })();
        match result {
            Err(error) => {
                self.ignore_metadata_fault(error)?;
                Ok(None)
            }
            result => result,
        }
    }

    fn marked_id(&self, dictionary: Option<&Dictionary>) -> Result<Option<i64>> {
        let Some(value) = dictionary.and_then(|d| d.get(b"MCID")) else {
            return Ok(None);
        };
        match self.document().resolve(value) {
            Ok(value) => Ok(value.as_integer().filter(|n| *n >= 0)),
            Err(error) if metadata_fault(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Reports malformed marked-content metadata as a repair; other errors
    /// stand.
    fn ignore_metadata_fault(&self, error: Error) -> Result<()> {
        if !metadata_fault(&error) {
            return Err(error);
        }
        let mut diagnostic =
            Diagnostic::new(pagura_extract::repairs::MARKED_PROPERTY, error.message)
                .at(self.position);
        if let Position::Stream { object, .. } = self.position {
            diagnostic = diagnostic.in_object(object);
        }
        self.document().repair(diagnostic)
    }
}

fn metadata_fault(error: &Error) -> bool {
    matches!(
        error.kind,
        ErrorKind::Syntax | ErrorKind::MissingObject | ErrorKind::Cycle
    )
}
