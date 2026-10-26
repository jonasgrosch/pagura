//! Default optional-content configuration (ISO 32000-2, 8.11).

use crate::{features, repairs};
use pagura_core::{Dictionary, Error, ObjectId, Result, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub(crate) struct Visibility {
    configuration: Configuration,
    groups: BTreeMap<ObjectId, bool>,
    applicable: BTreeSet<ObjectId>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum Configuration {
    #[default]
    Absent,
    Ignored,
    Active,
}

impl Visibility {
    pub(crate) fn new(document: &Document) -> ReadResult<Self> {
        Self::read(document).map_err(|error| {
            document
                .trailer()
                .get(b"Root")
                .and_then(Value::as_reference)
                .map_or(error.clone(), |id| error.in_object(id))
        })
    }
    fn read(document: &Document) -> ReadResult<Self> {
        let Some(properties) = document.catalog()?.get(b"OCProperties") else {
            return Ok(Self::default());
        };
        let properties = document.resolve_dictionary(properties)?;
        let Some(configuration) = properties.get(b"D") else {
            configuration_repair(
                document,
                repairs::OPTIONAL_CONFIG,
                "OCProperties has no default configuration",
            )?;
            return Ok(Self {
                configuration: Configuration::Ignored,
                ..Self::default()
            });
        };
        let configuration = document.resolve_dictionary(configuration)?;
        validate_default(document, configuration)?;
        let Some(values) = properties.get(b"OCGs") else {
            configuration_repair(
                document,
                repairs::OPTIONAL_GROUPS,
                "OCProperties has no OCGs array",
            )?;
            return Ok(Self {
                configuration: Configuration::Ignored,
                ..Self::default()
            });
        };
        let values = document
            .resolve(values)?
            .as_array()
            .ok_or(Error::syntax("OCProperties has no OCGs array"))?;
        if values.len() > document.limits().value_nodes {
            return Err(Error::limit("optional group count budget exceeded").into());
        }
        let mut groups = BTreeMap::new();
        let mut applicable = BTreeSet::new();
        for value in values {
            let id = value
                .as_reference()
                .ok_or(Error::syntax("OCG must be indirect"))?;
            let dictionary = document.resolve_dictionary(value)?;
            if !dictionary.is_name(b"Type", b"OCG") {
                return Err(Error::syntax("invalid optional content group").into());
            }
            if view_intent(document, dictionary)? {
                applicable.insert(id);
            }
            if groups.insert(id, true).is_some() {
                return Err(Error::syntax("duplicate optional content group").into());
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
                return Err(Error::syntax("group occurs in ON and OFF").into());
            }
            *group = state;
        }
        let mut visibility = Self {
            configuration: Configuration::Active,
            groups,
            applicable,
        };
        visibility.apply_usage(document, configuration)?;
        Ok(visibility)
    }

    /// View is the engine's initial viewing event. Export/Print applications
    /// do not override it. Missing OCGs means no affected groups (Table 101).
    /// Recommendations are ANDed across all matching AS entries (8.11.4.4).
    fn apply_usage(&mut self, document: &Document, configuration: &Dictionary) -> ReadResult<()> {
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
                return Err(Error::syntax("invalid usage application event").into());
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
                    return Err(Error::syntax("usage application references unknown group").into());
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

    /// Other applications may use the OC tag without optional-content meaning
    /// (8.11.3.2). XObject/annotation OC entries still use the strict entry point.
    pub(crate) fn marked(&self, document: &Document, value: &Value) -> ReadResult<bool> {
        if self.configuration == Configuration::Ignored {
            return Ok(true);
        }
        let d = document.resolve_dictionary(value)?;
        if d.is_name(b"Type", b"OCMD")
            || (d.is_name(b"Type", b"OCG")
                && value
                    .as_reference()
                    .is_some_and(|id| self.groups.contains_key(&id)))
        {
            self.visible(document, value)
        } else {
            Ok(true)
        }
    }
    pub(crate) fn visible(&self, document: &Document, value: &Value) -> ReadResult<bool> {
        // 8.11.4.2 explicitly requires ignoring these structures when
        // OCProperties is absent, including negated membership expressions.
        if self.configuration == Configuration::Absent {
            let mut diagnostic = pagura_core::Diagnostic::new(
                repairs::OPTIONAL_PROPERTIES,
                "optional content requires OCProperties",
            );
            if let Some(id) = value.as_reference() {
                diagnostic = diagnostic.in_object(id);
            }
            document.repair(diagnostic)?;
            return Ok(true);
        }
        if self.configuration == Configuration::Ignored {
            return Ok(true);
        }
        let dictionary = document.resolve_dictionary(value)?;
        match dictionary
            .get_name(b"Type")
            .map(pagura_core::Name::as_bytes)
        {
            Some(b"OCG") => {
                let id = value
                    .as_reference()
                    .ok_or(Error::syntax("OCG must be indirect"))?;
                Ok(self.group(document, &Value::Reference(id))?.unwrap_or(true))
            }
            Some(b"OCMD") => self.membership(document, dictionary),
            _ => Err(Error::syntax("optional content must be OCG or OCMD").into()),
        }
    }

    fn membership(&self, document: &Document, dictionary: &Dictionary) -> ReadResult<bool> {
        let mut work = 0;
        if let Some(expression) = dictionary.get(b"VE") {
            return Ok(self
                .expression(document, expression, 0, &mut work)?
                .unwrap_or(true));
        }
        let Some(members) = dictionary.get(b"OCGs") else {
            return Ok(true);
        };
        let resolved = document.resolve(members)?;
        let members = resolved
            .as_array()
            .unwrap_or_else(|| std::slice::from_ref(members));
        let mut count = 0;
        let mut any_on = false;
        let mut all_on = true;
        for member in members {
            if matches!(document.resolve(member)?, Value::Null) {
                continue;
            }
            let Some(state) = self.group(document, member)? else {
                continue;
            };
            count += 1;
            any_on |= state;
            all_on &= state;
        }
        if count == 0 {
            return Ok(true);
        }
        match name(document, dictionary, b"P")? {
            None | Some(b"AnyOn") => Ok(any_on),
            Some(b"AllOn") => Ok(all_on),
            Some(b"AnyOff") => Ok(!all_on),
            Some(b"AllOff") => Ok(!any_on),
            _ => Err(Error::syntax("invalid optional content policy").into()),
        }
    }

    fn expression(
        &self,
        document: &Document,
        value: &Value,
        depth: usize,
        work: &mut usize,
    ) -> ReadResult<Option<bool>> {
        if depth >= document.limits().depth || *work >= document.limits().value_nodes {
            return Err(Error::limit("optional expression budget exceeded").into());
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
            return Err(Error::syntax("wrong visibility expression arity").into());
        }
        let mut count = 0;
        let mut any = false;
        let mut all = true;
        // Evaluate all branches: malformed/unsupported input cannot hide behind
        // short-circuit evaluation in an expression that happens to be false.
        for argument in arguments {
            let Some(state) = self.expression(document, argument, depth + 1, work)? else {
                continue;
            };
            count += 1;
            any |= state;
            all &= state;
        }
        if !matches!(operator.as_bytes(), b"And" | b"Or" | b"Not") {
            return Err(Error::syntax("unknown visibility expression operator").into());
        }
        if count == 0 {
            return Ok(None);
        }
        match operator.as_bytes() {
            b"And" => Ok(Some(all)),
            b"Or" => Ok(Some(any)),
            b"Not" => Ok(Some(!any)),
            _ => Err(Error::syntax("unknown visibility expression operator").into()),
        }
    }

    fn group(&self, document: &Document, value: &Value) -> ReadResult<Option<bool>> {
        let id = value.as_reference().ok_or(Error::syntax(
            "visibility expression requires an OCG reference",
        ))?;
        let dictionary = document.resolve_dictionary(value)?;
        if !dictionary.is_name(b"Type", b"OCG") {
            return Err(Error::syntax("visibility expression member is not OCG").into());
        }
        if let Some(&state) = self.groups.get(&id) {
            return Ok(self.applicable.contains(&id).then_some(state));
        }
        document.repair(
            pagura_core::Diagnostic::new(
                repairs::OPTIONAL_GROUP,
                "undefined optional content group",
            )
            .in_object(id),
        )?;
        Ok(view_intent(document, dictionary)?.then_some(true))
    }
}

fn configuration_repair(
    document: &Document,
    tag: pagura_core::Repair,
    message: &'static str,
) -> Result<()> {
    let mut diagnostic = pagura_core::Diagnostic::new(tag, message);
    if let Some(id) = document
        .trailer()
        .get(b"Root")
        .and_then(Value::as_reference)
    {
        diagnostic = diagnostic.in_object(id);
    }
    document.repair(diagnostic)
}

fn list(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> ReadResult<BTreeSet<ObjectId>> {
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
) -> ReadResult<Option<&'a [u8]>> {
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
) -> ReadResult<Option<bool>> {
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
                return Err(Error::syntax("invalid usage zoom range").into());
            }
            return Ok(Some(minimum <= 1.0 && 1.0 < maximum));
        }
        _ => {
            return Err(Error::unsupported(
                features::OPTIONALCONTENT_USAGE,
                "usage category requires viewer context not provided by this API",
            )
            .into());
        }
    };
    match name(document, dictionary, key)? {
        Some(b"ON") => Ok(Some(true)),
        Some(b"OFF") => Ok(Some(false)),
        None if category == b"Print" => Ok(None),
        _ => Err(Error::syntax("missing or invalid usage state").into()),
    }
}

fn validate_default(document: &Document, config: &Dictionary) -> ReadResult<()> {
    // Default configuration constraints are stricter than other configurations
    // (8.11.4.3, Table 99); OFF arrays still choose initially hidden groups.
    if name(document, config, b"BaseState")?.is_some_and(|v| v != b"ON") {
        return Err(Error::syntax("default BaseState must be ON").into());
    }
    if let Some(intent) = config.get(b"Intent")
        && document
            .resolve(intent)?
            .as_name()
            .is_none_or(|v| v != b"View")
    {
        return Err(Error::syntax("default Intent must be View").into());
    }
    Ok(())
}
fn view_intent(document: &Document, group: &Dictionary) -> ReadResult<bool> {
    let Some(intent) = group.get(b"Intent") else {
        return Ok(true);
    };
    let intent = document.resolve(intent)?;
    let values = intent
        .as_array()
        .unwrap_or_else(|| std::slice::from_ref(intent));
    let mut applies = false;
    for value in values {
        let intent = document
            .resolve(value)?
            .as_name()
            .ok_or(Error::syntax("OCG intent must be a name"))?;
        applies |= intent == b"View";
    }
    Ok(applies)
}
