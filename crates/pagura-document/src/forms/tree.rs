//! Iterative field walk. Forward Fields/Kids own membership (12.7.4.1).
use super::{
    AcroForm, Field, FieldFlags, FieldKind,
    attributes::{self, Entry, Inherited},
    names, values, widgets,
};
use crate::{
    Document,
    navigation::context::{Context, Source, identity},
    tags,
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
    ctx: Context<'a>,
    form: AcroForm<'a>,
    seen: BTreeSet<usize>,
    names: BTreeMap<String, Inherited<'a>>,
}
pub(super) fn read(doc: &Document) -> Result<Option<AcroForm<'_>>> {
    let mut ctx = Context::new(doc);
    let (catalog, source) = ctx.catalog()?;
    let Some(value) = ctx.present(catalog, b"AcroForm", source)? else {
        return Ok(None);
    };
    let (d, source) = ctx.dictionary(value, source)?;
    let inherited = Inherited::defaults(&mut ctx, d, source)?;
    let form = header(&mut ctx, d, source, &inherited)?;
    let roots = if let Some(fields) = ctx.present(d, b"Fields", source)? {
        attributes::array(&mut ctx, fields, source)?
    } else {
        ctx.repair(
            tags::FORM_FIELDS,
            "absent Fields read as an empty field tree (12.7.3, Table 224)",
            source,
        )?;
        &[]
    };
    ctx.allocate::<Pending<'_>>(roots.len(), source)?;
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
        ctx,
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
    ctx: &mut Context<'a>,
    d: &'a Dictionary,
    source: Source,
    inherited: &Inherited<'a>,
) -> Result<AcroForm<'a>> {
    Ok(AcroForm {
        fields: Vec::new(),
        widgets: Vec::new(),
        need_appearances: ctx
            .entry(d, b"NeedAppearances", source)?
            .map(|v| {
                v.as_bool()
                    .ok_or_else(|| source.error(Error::syntax("NeedAppearances must be boolean")))
            })
            .transpose()?
            .unwrap_or(false),
        signature_flags: ctx
            .present(d, b"SigFlags", source)?
            .map(|value| attributes::unsigned(Entry { value, source }, ctx))
            .transpose()?
            .unwrap_or(0),
        calculation_order: ctx
            .present(d, b"CO", source)?
            .map(|v| attributes::array(ctx, v, source))
            .transpose()?
            .unwrap_or(&[]),
        default_resources: ctx
            .present(d, b"DR", source)?
            .map(|v| ctx.dictionary(v, source).map(|(d, _)| d))
            .transpose()?,
        default_appearance: attributes::appearance(ctx, inherited.get(b"DA"))?,
        justification: attributes::quadding(ctx, inherited.get(b"Q"))?,
        has_xfa: ctx.present(d, b"XFA", source)?.is_some(),
        dictionary: d,
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
        self.ctx.step(depth, source)?;
        let indirect = value.as_reference().is_some();
        let (value, source) = self.ctx.resolve(value, source)?;
        self.ctx.allocate::<usize>(8, source)?;
        if !self.seen.insert(identity(value)) {
            return self.ctx.repair(
                tags::FORM_CYCLE,
                "repeated field or widget edge omitted (12.7.4.1)",
                source,
            );
        }
        let (d, source) = self.ctx.dictionary(value, source)?;
        if !indirect {
            self.ctx.repair(
                tags::FORM_STRUCTURE,
                "direct field or widget dictionary read in place (12.7.4.1)",
                source,
            )?;
        }
        self.parent(d, parent, source)?;
        // Unnamed children are widget representations, not fields (12.7.4.2).
        let has_name = self.ctx.present(d, b"T", source)?.is_some();
        if let Some(field) = parent.filter(|_| !has_name) {
            self.form
                .widgets
                .push(widgets::read(&mut self.ctx, d, source, field, false)?);
            return Ok(());
        }
        let attrs = inherited.overlay(&mut self.ctx, d, source)?;
        let field = self.field(d, source, parent, &attrs)?;
        let index = self.form.fields.len();
        self.form.fields.push(field);
        if widgets::is_widget(&mut self.ctx, d, source)? {
            self.form
                .widgets
                .push(widgets::read(&mut self.ctx, d, source, index, true)?);
        }
        let Some(kids) = self.ctx.present(d, b"Kids", source)? else {
            return Ok(());
        };
        let kids = attributes::array(&mut self.ctx, kids, source)?;
        let mut has_fields = false;
        let mut has_widgets = false;
        self.ctx.allocate::<Pending<'_>>(kids.len(), source)?;
        for value in kids.iter().rev() {
            self.ctx.step(depth + 1, source)?;
            let (child, _) = self.ctx.dictionary(value, source)?;
            let is_field = self.ctx.present(child, b"T", source)?.is_some();
            has_fields |= is_field;
            has_widgets |= !is_field;
            stack.push(Pending {
                value,
                source,
                parent: Some(index),
                inherited: attrs,
                depth: depth + 1,
            });
        }
        if has_fields {
            if has_widgets || widgets::is_widget(&mut self.ctx, d, source)? {
                self.ctx.repair(
                    tags::FORM_STRUCTURE,
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
    fn parent(&mut self, d: &'a Dictionary, parent: Option<usize>, source: Source) -> Result<()> {
        let expected = parent.map(|i| self.form.fields[i].dictionary);
        let actual = self
            .ctx
            .present(d, b"Parent", source)?
            .map(|v| self.ctx.resolve(v, source))
            .transpose()?;
        let matches = match (expected, actual) {
            (None, None) => true,
            (Some(expected), Some((Value::Dictionary(actual), _))) => {
                std::ptr::eq(expected, actual)
            }
            _ => false,
        };
        if !matches {
            self.ctx.repair(
                tags::FORM_PARENT,
                "Fields and Kids membership overrides inconsistent Parent (12.7.4.1)",
                source,
            )?;
        }
        Ok(())
    }
    fn field(
        &mut self,
        d: &'a Dictionary,
        source: Source,
        parent: Option<usize>,
        attrs: &Inherited<'a>,
    ) -> Result<Field<'a>> {
        self.ctx.allocate::<Field<'_>>(1, source)?;
        let partial_name = attributes::partial_name(&mut self.ctx, d, source)?;
        if partial_name.as_ref().is_none_or(|s| s.contains('.')) {
            self.ctx.repair(
                tags::FORM_NAME,
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
            .filter(|n| *n <= self.ctx.doc.limits().string_bytes)
            .ok_or_else(|| {
                source.error(Error::limit("qualified field name exceeds string budget"))
            })?;
        self.ctx.allocate::<u8>(length, source)?;
        let mut qualified_name = String::with_capacity(length);
        qualified_name.push_str(prefix);
        if separator != 0 {
            qualified_name.push('.');
        }
        qualified_name.push_str(part);
        self.duplicate(&qualified_name, attrs, source)?;
        let flags = FieldFlags(
            attrs
                .get(b"Ff")
                .map(|e| attributes::unsigned(e, &mut self.ctx))
                .transpose()?
                .unwrap_or(0),
        );
        // Nonterminal dictionaries need no FT. Kids are inspected using their
        // own field entries, never an inherited FT (12.7.4.1–12.7.4.2).
        let group = self.has_field_children(d, source)?;
        let kind = if group {
            FieldKind::Group
        } else {
            values::kind(&mut self.ctx, d, attrs, flags, source)?
        };
        Ok(Field {
            object: source.object,
            position: source.position,
            parent,
            partial_name,
            qualified_name,
            alternative_name: attributes::optional_text(&mut self.ctx, d, b"TU", source)?,
            mapping_name: attributes::optional_text(&mut self.ctx, d, b"TM", source)?,
            flags,
            value: if group {
                None
            } else {
                values::read(&mut self.ctx, &kind, attrs.get(b"V"))?
            },
            default_value: if group {
                None
            } else {
                values::read(&mut self.ctx, &kind, attrs.get(b"DV"))?
            },
            kind,
            default_appearance: attributes::appearance(&mut self.ctx, attrs.get(b"DA"))?,
            justification: attributes::quadding(&mut self.ctx, attrs.get(b"Q"))?,
            additional_actions: attributes::additional(&mut self.ctx, d, source)?,
            dictionary: d,
        })
    }
    fn has_field_children(&mut self, d: &'a Dictionary, source: Source) -> Result<bool> {
        let Some(kids) = self.ctx.present(d, b"Kids", source)? else {
            return Ok(false);
        };
        for value in attributes::array(&mut self.ctx, kids, source)? {
            self.ctx.step(0, source)?;
            let (child, _) = self.ctx.dictionary(value, source)?;
            if self.ctx.present(child, b"T", source)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn duplicate(&mut self, name: &str, attrs: &Inherited<'a>, source: Source) -> Result<()> {
        if let Some(previous) = self.names.get(name).copied() {
            let field_type = values::field_type(&mut self.ctx, attrs)?;
            let same_type = field_type == values::field_type(&mut self.ctx, &previous)?;
            let text_strings = field_type.is_some_and(|n| matches!(n.as_bytes(), b"Tx" | b"Ch"));
            let mut same_values = true;
            for key in [b"V".as_slice(), b"DV"] {
                let resolve = |ctx: &mut Context<'a>, attrs: &Inherited<'a>| {
                    attrs
                        .get(key)
                        .map(|e| ctx.resolve(e.value, e.source).map(|(v, _)| v))
                        .transpose()
                };
                let a = resolve(&mut self.ctx, attrs)?;
                let b = resolve(&mut self.ctx, &previous)?;
                same_values &= names::equivalent(&mut self.ctx, a, b, source, text_strings)?;
            }
            if !same_type || !same_values {
                self.ctx.repair(
                    tags::FORM_DUPLICATE_NAME,
                    "conflicting duplicate field names retained as separate objects (12.7.4.2)",
                    source,
                )?;
            }
        } else {
            self.ctx.allocate::<u8>(name.len(), source)?;
            self.ctx.allocate::<(String, Inherited<'_>)>(2, source)?;
            self.names.insert(name.to_owned(), *attrs);
        }
        Ok(())
    }
}
