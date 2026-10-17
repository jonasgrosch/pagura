//! Default optional-content configuration (ISO 32000-2, 8.11).

use crate::features;
use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
use pagura_document::Document;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub(crate) struct Visibility {
    groups: BTreeMap<ObjectId, bool>,
}

impl Visibility {
    pub(crate) fn new(document: &Document) -> Result<Self> {
        let Some(properties) = document.catalog()?.get(b"OCProperties") else {
            return Ok(Self::default());
        };
        let properties = document.resolve_dictionary(properties)?;
        let configuration = properties
            .get(b"D")
            .ok_or(Error::syntax("OCProperties has no default configuration"))?;
        let configuration = document.resolve_dictionary(configuration)?;
        if let Some(usage) = configuration.get(b"AS")
            && !document
                .resolve(usage)?
                .as_array()
                .is_some_and(<[Value]>::is_empty)
        {
            return Err(Error::unsupported(
                features::OPTIONALCONTENT_USAGE,
                "automatic usage configuration is not implemented",
            ));
        }
        let base = match name(document, configuration, b"BaseState")? {
            None | Some(b"ON" | b"Unchanged") => true,
            Some(b"OFF") => false,
            _ => return Err(Error::syntax("invalid optional content BaseState")),
        };
        let values = properties
            .get(b"OCGs")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_array)
            .ok_or(Error::syntax("OCProperties has no OCGs array"))?;
        if values.len() > document.limits().value_nodes {
            return Err(Error::limit("optional group count budget exceeded"));
        }
        let mut groups = BTreeMap::new();
        for value in values {
            let id = value
                .as_reference()
                .ok_or(Error::syntax("OCG must be indirect"))?;
            let dictionary = document.resolve_dictionary(value)?;
            if !dictionary.is_name(b"Type", b"OCG") {
                return Err(Error::syntax("invalid optional content group"));
            }
            if groups.insert(id, base).is_some() {
                return Err(Error::syntax("duplicate optional content group"));
            }
        }
        let on = list(document, configuration, b"ON")?;
        let off = list(document, configuration, b"OFF")?;
        for (&id, state) in on
            .iter()
            .map(|id| (id, true))
            .chain(off.iter().map(|id| (id, false)))
        {
            let group = groups
                .get_mut(&id)
                .ok_or(Error::syntax("configuration references undefined group"))?;
            if state && off.contains(&id) {
                return Err(Error::syntax("group occurs in ON and OFF"));
            }
            *group = state;
        }
        Ok(Self { groups })
    }

    pub(crate) fn visible(&self, document: &Document, value: &Value) -> Result<bool> {
        let dictionary = document.resolve_dictionary(value)?;
        match dictionary
            .get_name(b"Type")
            .map(pagura_core::Name::as_bytes)
        {
            Some(b"OCG") => {
                let id = value
                    .as_reference()
                    .ok_or(Error::syntax("OCG must be indirect"))?;
                self.groups
                    .get(&id)
                    .copied()
                    .ok_or(Error::syntax("undefined optional content group"))
            }
            Some(b"OCMD") => self.membership(document, dictionary),
            _ => Err(Error::syntax("optional content must be OCG or OCMD")),
        }
    }

    fn membership(&self, document: &Document, dictionary: &Dictionary) -> Result<bool> {
        let mut work = 0;
        if let Some(expression) = dictionary.get(b"VE") {
            return self.expression(document, expression, 0, &mut work);
        }
        let members = dictionary
            .get(b"OCGs")
            .ok_or(Error::syntax("OCMD has no OCGs or VE"))?;
        let resolved = document.resolve(members)?;
        let members = resolved
            .as_array()
            .unwrap_or_else(|| std::slice::from_ref(members));
        if members.is_empty() {
            return Err(Error::syntax("OCMD has empty OCGs"));
        }
        let mut any_on = false;
        let mut all_on = true;
        for member in members {
            let state = self.group(document, member)?;
            any_on |= state;
            all_on &= state;
        }
        match name(document, dictionary, b"P")? {
            None | Some(b"AnyOn") => Ok(any_on),
            Some(b"AllOn") => Ok(all_on),
            Some(b"AnyOff") => Ok(!all_on),
            Some(b"AllOff") => Ok(!any_on),
            _ => Err(Error::syntax("invalid optional content policy")),
        }
    }

    fn expression(
        &self,
        document: &Document,
        value: &Value,
        depth: usize,
        work: &mut usize,
    ) -> Result<bool> {
        if depth >= document.limits().depth || *work >= document.limits().value_nodes {
            return Err(Error::limit("optional expression budget exceeded"));
        }
        *work += 1;
        let value_resolved = document.resolve(value)?;
        let Some(array) = value_resolved.as_array() else {
            return self.group(document, value);
        };
        let operator = array
            .first()
            .and_then(Value::as_name)
            .ok_or(Error::syntax("visibility expression has no operator"))?;
        let arguments = array
            .get(1..)
            .ok_or(Error::syntax("invalid visibility expression"))?;
        if arguments.is_empty() || (operator == b"Not" && arguments.len() != 1) {
            return Err(Error::syntax("wrong visibility expression arity"));
        }
        let mut any = false;
        let mut all = true;
        // Evaluate all branches: malformed/unsupported input cannot hide behind
        // short-circuit evaluation in an expression that happens to be false.
        for argument in arguments {
            let state = self.expression(document, argument, depth + 1, work)?;
            any |= state;
            all &= state;
        }
        match operator.as_bytes() {
            b"And" => Ok(all),
            b"Or" => Ok(any),
            b"Not" => Ok(!any),
            _ => Err(Error::syntax("unknown visibility expression operator")),
        }
    }

    fn group(&self, document: &Document, value: &Value) -> Result<bool> {
        let id = value.as_reference().ok_or(Error::syntax(
            "visibility expression requires an OCG reference",
        ))?;
        if !document.resolve_dictionary(value)?.is_name(b"Type", b"OCG") {
            return Err(Error::syntax("visibility expression member is not OCG"));
        }
        self.groups
            .get(&id)
            .copied()
            .ok_or(Error::syntax("undefined optional content group"))
    }
}

fn list(document: &Document, dictionary: &Dictionary, key: &[u8]) -> Result<BTreeSet<ObjectId>> {
    let Some(value) = dictionary.get(key) else {
        return Ok(BTreeSet::new());
    };
    document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax(
            "optional configuration list must be an array",
        ))?
        .iter()
        .map(|value| {
            value.as_reference().ok_or(Error::syntax(
                "optional configuration must reference groups",
            ))
        })
        .collect()
}

fn name<'a>(
    document: &'a Document,
    dictionary: &'a Dictionary,
    key: &[u8],
) -> Result<Option<&'a [u8]>> {
    dictionary
        .get(key)
        .map(|v| {
            document
                .resolve(v)?
                .as_name()
                .map(pagura_core::Name::as_bytes)
                .ok_or(Error::syntax("optional content entry must be a name"))
        })
        .transpose()
}
