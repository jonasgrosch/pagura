//! Iterative field walk. Forward Fields/Kids own membership (12.7.4.1).
use super::{
    AcroForm, Field, FieldFlags, FieldKind,
    attributes::{self, Entry, Inherited, Key},
    names, values, widgets,
};
use crate::{
    Document, repairs,
    view::{Context, Source, ValueIdentity, identity},
};
use pagura_core::{Dictionary, Error, Result, Value};
use std::collections::{BTreeMap, BTreeSet};

struct Pending<'a> {
    value: &'a Value,
    source: Source,
    parent: Option<usize>,
    inherited: Inherited<'a>,
    depth: usize,
}

struct Walk<'a> {
    context: Context<'a>,
    form: AcroForm<'a>,
    seen: BTreeSet<ValueIdentity>,
    names: BTreeMap<String, Inherited<'a>>,
}

pub(super) fn read(document: &Document) -> Result<Option<AcroForm<'_>>> {
    let mut context = Context::new(document);
    let (catalog, source) = context.catalog()?;
    let Some(value) = context.present(catalog, b"AcroForm", source)? else {
        return Ok(None);
    };
    let (dictionary, source) = context.dictionary(value, source)?;
    let inherited = Inherited::defaults(&mut context, dictionary, source)?;
    let form = header(&mut context, dictionary, source, &inherited)?;
    let roots = if let Some(fields) = context.present(dictionary, b"Fields", source)? {
        attributes::array(&mut context, fields, source)?
    } else {
        context.repair(
            repairs::FORM_FIELDS,
            "absent Fields read as an empty field tree (12.7.3, Table 224)",
            source,
        )?;
        &[]
    };
    context.allocate::<Pending<'_>>(roots.len(), source)?;
    let mut stack: Vec<_> = roots
        .iter()
        .rev()
        .map(|value| Pending {
            value,
            source,
            parent: None,
            inherited,
            depth: 0,
        })
        .collect();
    let mut walk = Walk {
        context,
        form,
        seen: BTreeSet::new(),
        names: BTreeMap::new(),
    };
    while let Some(pending) = stack.pop() {
        walk.node(&pending, &mut stack)?;
    }
    Ok(Some(walk.form))
}

fn header<'a>(
    context: &mut Context<'a>,
    dictionary: &'a Dictionary,
    source: Source,
    inherited: &Inherited<'a>,
) -> Result<AcroForm<'a>> {
    Ok(AcroForm {
        fields: Vec::new(),
        widgets: Vec::new(),
        need_appearances: context
            .entry(dictionary, b"NeedAppearances", source)?
            .map(|v| {
                v.as_bool()
                    .ok_or_else(|| source.error(Error::syntax("NeedAppearances must be boolean")))
            })
            .transpose()?
            .unwrap_or(false),
        signature_flags: context
            .present(dictionary, b"SigFlags", source)?
            .map(|value| attributes::unsigned(Entry { value, source }, context))
            .transpose()?
            .unwrap_or(0),
        calculation_order: context
            .present(dictionary, b"CO", source)?
            .map(|v| attributes::array(context, v, source))
            .transpose()?
            .unwrap_or(&[]),
        default_resources: context
            .present(dictionary, b"DR", source)?
            .map(|v| {
                context
                    .dictionary(v, source)
                    .map(|(dictionary, _)| dictionary)
            })
            .transpose()?,
        default_appearance: attributes::appearance(context, inherited.get(Key::Da))?,
        justification: attributes::quadding(context, inherited.get(Key::Q))?,
        has_xfa: context.present(dictionary, b"XFA", source)?.is_some(),
        dictionary,
    })
}

impl<'a> Walk<'a> {
    fn node(&mut self, pending: &Pending<'a>, stack: &mut Vec<Pending<'a>>) -> Result<()> {
        let Pending {
            value,
            source,
            parent,
            inherited,
            depth,
        } = *pending;
        self.context.step(depth, source)?;
        let indirect = value.as_reference().is_some();
        let (value, source) = self.context.resolve(value, source)?;
        self.context.index_entry(source)?;
        if !self.seen.insert(identity(value)) {
            return self.context.repair(
                repairs::FORM_CYCLE,
                "repeated field or widget edge omitted (12.7.4.1)",
                source,
            );
        }
        let (dictionary, source) = self.context.dictionary(value, source)?;
        if !indirect {
            self.context.repair(
                repairs::FORM_STRUCTURE,
                "direct field or widget dictionary read in place (12.7.4.1)",
                source,
            )?;
        }
        self.parent(dictionary, parent, source)?;
        // Unnamed children are widget representations, not fields (12.7.4.2).
        let has_name = self.context.present(dictionary, b"T", source)?.is_some();
        if let Some(field) = parent.filter(|_| !has_name) {
            self.form.widgets.push(widgets::read(
                &mut self.context,
                dictionary,
                source,
                field,
                widgets::Representation::Separate,
            )?);
            return Ok(());
        }
        let effective = inherited.overlay(&mut self.context, dictionary, source)?;
        let field = self.field(dictionary, source, parent, &effective)?;
        let index = self.form.fields.len();
        self.form.fields.push(field);
        if widgets::is_widget(&mut self.context, dictionary, source)? {
            self.form.widgets.push(widgets::read(
                &mut self.context,
                dictionary,
                source,
                index,
                widgets::Representation::Merged,
            )?);
        }
        let Some(kids) = self.context.present(dictionary, b"Kids", source)? else {
            return Ok(());
        };
        let kids = attributes::array(&mut self.context, kids, source)?;
        let mut has_fields = false;
        let mut has_widgets = false;
        self.context.allocate::<Pending<'_>>(kids.len(), source)?;
        for value in kids.iter().rev() {
            self.context.step(depth + 1, source)?;
            let (child, _) = self.context.dictionary(value, source)?;
            let is_field = self.context.present(child, b"T", source)?.is_some();
            has_fields |= is_field;
            has_widgets |= !is_field;
            stack.push(Pending {
                value,
                source,
                parent: Some(index),
                inherited: effective,
                depth: depth + 1,
            });
        }
        if has_fields {
            if has_widgets || widgets::is_widget(&mut self.context, dictionary, source)? {
                self.context.repair(
                    repairs::FORM_STRUCTURE,
                    "mixed field and widget children retained separately (12.7.4.1)",
                    source,
                )?;
            }
            self.form.fields[index].kind = FieldKind::Group;
            self.form.fields[index].value = None;
            self.form.fields[index].default_value = None;
        }
        Ok(())
    }

    fn parent(
        &mut self,
        dictionary: &'a Dictionary,
        parent: Option<usize>,
        source: Source,
    ) -> Result<()> {
        let expected = parent.map(|i| self.form.fields[i].dictionary);
        let actual = self
            .context
            .present(dictionary, b"Parent", source)?
            .map(|v| self.context.resolve(v, source))
            .transpose()?;
        let matches = match (expected, actual) {
            (None, None) => true,
            (Some(expected), Some((Value::Dictionary(actual), _))) => {
                std::ptr::eq(expected, actual)
            }
            _ => false,
        };
        if !matches {
            self.context.repair(
                repairs::FORM_PARENT,
                "Fields and Kids membership overrides inconsistent Parent (12.7.4.1)",
                source,
            )?;
        }
        Ok(())
    }

    fn field(
        &mut self,
        dictionary: &'a Dictionary,
        source: Source,
        parent: Option<usize>,
        effective: &Inherited<'a>,
    ) -> Result<Field<'a>> {
        self.context.allocate::<Field<'_>>(1, source)?;
        let partial_name = attributes::partial_name(&mut self.context, dictionary, source)?;
        if partial_name.as_ref().is_none_or(|s| s.contains('.')) {
            self.context.repair(
                repairs::FORM_NAME,
                "anonymous or malformed field name retained verbatim (12.7.4.2)",
                source,
            )?;
        }
        let prefix = parent.map_or("", |i| self.form.fields[i].qualified_name.as_str());
        let part = partial_name.as_deref().unwrap_or("");
        let separator = usize::from(parent.is_some() && partial_name.is_some());
        let length = prefix
            .len()
            .checked_add(part.len())
            .and_then(|n| n.checked_add(separator))
            .filter(|n| *n <= self.context.document.limits().string_bytes)
            .ok_or_else(|| {
                source.error(Error::limit("qualified field name exceeds string budget"))
            })?;
        self.context.allocate::<u8>(length, source)?;
        let mut qualified_name = String::with_capacity(length);
        qualified_name.push_str(prefix);
        if separator != 0 {
            qualified_name.push('.');
        }
        qualified_name.push_str(part);
        self.duplicate(&qualified_name, effective, source)?;
        let flags = FieldFlags::from_bits(
            effective
                .get(Key::Ff)
                .map(|e| attributes::unsigned(e, &mut self.context))
                .transpose()?
                .unwrap_or(0),
        );
        // Nonterminal dictionaries need no FT. Kids are inspected using their
        // own field entries, never an inherited FT (12.7.4.1–12.7.4.2).
        let group = self.has_field_children(dictionary, source)?;
        let kind = if group {
            FieldKind::Group
        } else {
            values::kind(&mut self.context, dictionary, effective, flags, source)?
        };
        let value = if group {
            None
        } else {
            values::read(&mut self.context, &kind, effective.get(Key::V))?
        };
        let default_value = if group {
            None
        } else {
            values::read(&mut self.context, &kind, effective.get(Key::Dv))?
        };
        Ok(Field {
            object: source.object,
            position: source.position,
            parent,
            partial_name,
            qualified_name,
            alternative_name: attributes::optional_text(
                &mut self.context,
                dictionary,
                b"TU",
                source,
            )?,
            mapping_name: attributes::optional_text(&mut self.context, dictionary, b"TM", source)?,
            flags,
            value: value.map(|(value, _)| value),
            default_value: default_value.map(|(value, _)| value),
            kind,
            default_appearance: attributes::appearance(&mut self.context, effective.get(Key::Da))?,
            justification: attributes::quadding(&mut self.context, effective.get(Key::Q))?,
            additional_actions: attributes::additional(&mut self.context, dictionary, source)?,
            dictionary,
        })
    }

    fn has_field_children(&mut self, dictionary: &'a Dictionary, source: Source) -> Result<bool> {
        let Some(kids) = self.context.present(dictionary, b"Kids", source)? else {
            return Ok(false);
        };
        for value in attributes::array(&mut self.context, kids, source)? {
            self.context.step(0, source)?;
            let (child, _) = self.context.dictionary(value, source)?;
            if self.context.present(child, b"T", source)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn duplicate(&mut self, name: &str, effective: &Inherited<'a>, source: Source) -> Result<()> {
        if let Some(previous) = self.names.get(name).copied() {
            let field_type = values::field_type(&mut self.context, effective)?;
            let same_type = field_type == values::field_type(&mut self.context, &previous)?;
            let strings = if field_type.is_some_and(|n| matches!(n.as_bytes(), b"Tx" | b"Ch")) {
                names::Strings::Text
            } else {
                names::Strings::Bytes
            };
            let mut same_values = true;
            for key in [Key::V, Key::Dv] {
                let resolve = |context: &mut Context<'a>, effective: &Inherited<'a>| {
                    effective
                        .get(key)
                        .map(|e| context.resolve(e.value, e.source).map(|(v, _)| v))
                        .transpose()
                };
                let a = resolve(&mut self.context, effective)?;
                let b = resolve(&mut self.context, &previous)?;
                same_values &= names::equivalent(&mut self.context, a, b, source, strings)?;
            }
            if !same_type || !same_values {
                self.context.repair(
                    repairs::FORM_DUPLICATE_NAME,
                    "conflicting duplicate field names retained as separate objects (12.7.4.2)",
                    source,
                )?;
            }
        } else {
            self.context.allocate::<u8>(name.len(), source)?;
            self.context
                .allocate::<(String, Inherited<'_>)>(2, source)?;
            self.names.insert(name.to_owned(), *effective);
        }
        Ok(())
    }
}
