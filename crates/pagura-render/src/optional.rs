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
        let mut visibility = Self { groups };
        visibility.apply_usage(document, configuration)?;
        Ok(visibility)
    }

    /// View is the engine's initial viewing event. Export/Print applications
    /// do not override it. Missing OCGs means no affected groups (Table 103).
    /// Recommendations are ANDed across all matching AS entries (8.11.4.4).
    fn apply_usage(&mut self, document: &Document, configuration: &Dictionary) -> Result<()> {
        let Some(applications) = configuration.get(b"AS") else {
            return Ok(());
        };
        let applications = document
            .resolve(applications)?
            .as_array()
            .ok_or(Error::syntax("AS must be an array"))?;
        let mut recommendations = BTreeMap::new();
        let mut work = 0_usize;
        for application in applications {
            let application = document.resolve_dictionary(application)?;
            let event = name(document, application, b"Event")?
                .ok_or(Error::syntax("usage application has no Event"))?;
            let categories = application
                .get(b"Category")
                .map(|v| document.resolve(v))
                .transpose()?
                .and_then(Value::as_array)
                .ok_or(Error::syntax("usage application has no Category array"))?;
            if !matches!(event, b"View" | b"Print" | b"Export") {
                return Err(Error::syntax("invalid usage application event"));
            }
            for value in categories {
                document
                    .resolve(value)?
                    .as_name()
                    .ok_or(Error::syntax("usage category must be a name"))?;
            }
            if event != b"View" {
                continue;
            }
            for id in list(document, application, b"OCGs")? {
                if !self.groups.contains_key(&id) {
                    return Err(Error::syntax("usage application references unknown group"));
                }
                let reference = Value::Reference(id);
                let dictionary = document.resolve_dictionary(&reference)?;
                let usage = dictionary
                    .get(b"Usage")
                    .map(|v| document.resolve_dictionary(v))
                    .transpose()?;
                let mut recommendation = None;
                for category in categories {
                    work = work
                        .checked_add(1)
                        .filter(|&v| v <= document.limits().operations)
                        .ok_or(Error::limit("optional usage work exceeded"))?;
                    let category = document
                        .resolve(category)?
                        .as_name()
                        .ok_or(Error::syntax("usage category must be a name"))?;
                    if let Some(usage) = usage
                        && let Some(value) = usage.get(category.as_bytes())
                        && let Some(state) = usage_state(
                            document,
                            category.as_bytes(),
                            document.resolve_dictionary(value)?,
                        )?
                    {
                        recommendation = Some(recommendation.unwrap_or(true) && state);
                    }
                }
                if let Some(state) = recommendation {
                    *recommendations.entry(id).or_insert(true) &= state;
                }
            }
        }
        for (id, state) in recommendations {
            *self
                .groups
                .get_mut(&id)
                .ok_or(Error::syntax("usage application references unknown group"))? = state;
        }
        Ok(())
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

fn usage_state(
    document: &Document,
    category: &[u8],
    dictionary: &Dictionary,
) -> Result<Option<bool>> {
    let key = match category {
        b"View" => b"ViewState".as_slice(),
        b"Print" => b"PrintState",
        b"Export" => b"ExportState",
        b"Zoom" => {
            // Initial View context is 100% magnification. Rendering DPI is output
            // sampling density, not interactive viewer magnification.
            let minimum = dictionary
                .get(b"min")
                .map(|v| {
                    document
                        .resolve(v)?
                        .as_number()
                        .ok_or(Error::syntax("invalid usage zoom minimum"))
                })
                .transpose()?
                .unwrap_or(0.0);
            let maximum = dictionary
                .get(b"max")
                .map(|v| {
                    document
                        .resolve(v)?
                        .as_number()
                        .ok_or(Error::syntax("invalid usage zoom maximum"))
                })
                .transpose()?
                .unwrap_or(f64::INFINITY);
            if minimum < 0.0 || minimum > maximum {
                return Err(Error::syntax("invalid usage zoom range"));
            }
            return Ok(Some(minimum <= 1.0 && 1.0 < maximum));
        }
        _ => {
            return Err(Error::unsupported(
                features::OPTIONALCONTENT_USAGE,
                "usage category requires viewer context not provided by this API",
            ));
        }
    };
    match name(document, dictionary, key)? {
        Some(b"ON") => Ok(Some(true)),
        Some(b"OFF") => Ok(Some(false)),
        None if category == b"Print" => Ok(None),
        _ => Err(Error::syntax("missing or invalid usage state")),
    }
}
