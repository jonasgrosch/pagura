//! Role maps and the two standard namespaces (14.7.3; 14.8.6; Annex M).
use super::{BTreeSet, Budget, Dictionary, Document, Error, Name, Result, Value};

/// Standard structure namespace, distinct from the PDF file's version (14.8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureNamespace {
    /// The default standard namespace, including when NS is absent.
    Pdf17,
    /// The explicitly declared PDF 2.0 standard namespace.
    Pdf20,
}
pub(super) struct Types<'a> {
    roles: Option<&'a Dictionary>,
    namespaces: Vec<&'a Dictionary>,
}
impl<'a> Types<'a> {
    pub(super) fn new(
        doc: &'a Document,
        root: &'a Dictionary,
        budget: &mut Budget,
    ) -> Result<Self> {
        let roles = super::entry(doc, root, b"RoleMap")?
            .map(|v| doc.resolve_dictionary(v))
            .transpose()?;
        let namespaces = super::entry(doc, root, b"Namespaces")?
            .map(|v| {
                doc.resolve(v).and_then(|v| {
                    v.as_array()
                        .ok_or(Error::syntax("structure Namespaces must be an array"))
                })
            })
            .transpose()?
            .unwrap_or(&[]);
        budget.store(namespaces.len().saturating_mul(size_of::<&Dictionary>()))?;
        let namespaces = namespaces
            .iter()
            .map(|v| {
                budget.work(1)?;
                doc.resolve_dictionary(v)
            })
            .collect::<Result<_>>()?;
        Ok(Self { roles, namespaces })
    }
    pub(super) fn resolve(
        &self,
        doc: &Document,
        element: &Dictionary,
        role: &Name,
        budget: &mut Budget,
    ) -> Result<(StructureNamespace, Option<Name>)> {
        let (namespace, roles) = if let Some(value) = super::entry(doc, element, b"NS")? {
            let ns = doc.resolve_dictionary(value)?;
            budget.work(self.namespaces.len())?;
            if !self.namespaces.iter().any(|n| std::ptr::eq(*n, ns)) {
                return Err(Error::syntax(
                    "structure namespace is absent from Namespaces (14.8.6.2)",
                ));
            }
            if super::name(doc, ns, b"Type")?.is_some_and(|n| n.as_bytes() != b"Namespace") {
                return Err(Error::syntax("invalid namespace Type (Table 356)"));
            }
            if super::entry(doc, ns, b"RoleMapNS")?.is_some() {
                return Err(Error::unsupported(
                    super::NAMESPACE,
                    "text.structure-namespace: namespace-specific role maps are not implemented (14.8.6.2)",
                ));
            }
            let value = ns
                .get(b"NS")
                .map(|v| doc.resolve(v))
                .transpose()?
                .and_then(Value::as_string)
                .ok_or(Error::syntax("namespace NS must be a string"))?;
            budget.store(value.len())?;
            let name = pagura_text::decode_text_string(value, doc.limits())?;
            let namespace = match name.as_str() {
                "http://iso.org/pdf/ssn" => StructureNamespace::Pdf17,
                "http://iso.org/pdf2/ssn" => StructureNamespace::Pdf20,
                _ => {
                    return Err(Error::unsupported(
                        super::NAMESPACE,
                        "text.structure-namespace: nonstandard structure namespace is not implemented (14.7.4)",
                    ));
                }
            };
            (namespace, None)
        } else {
            (StructureNamespace::Pdf17, self.roles)
        };
        Ok((namespace, role_type(doc, role, roles, namespace, budget)?))
    }
}
fn role_type(
    doc: &Document,
    role: &Name,
    roles: Option<&Dictionary>,
    namespace: StructureNamespace,
    budget: &mut Budget,
) -> Result<Option<Name>> {
    let mut current = role;
    let mut seen = BTreeSet::new();
    // Map even a standard initial name. Valid cycles stop when recognised or
    // revisited, rather than being mistaken for cycles in the structure tree.
    loop {
        budget.work(1)?;
        if !seen.insert(current.as_bytes()) {
            return Ok(None);
        }
        if let Some(mapped) = roles
            .map(|r| super::entry(doc, r, current.as_bytes()))
            .transpose()?
            .flatten()
        {
            current = doc
                .resolve(mapped)?
                .as_name()
                .ok_or(Error::syntax("RoleMap target must be a name"))?;
        } else {
            return Ok(standard(current, namespace).then(|| current.clone()));
        }
        if standard(current, namespace) {
            return Ok(Some(current.clone()));
        }
        if seen.len() > budget.limits.depth {
            return Err(Error::limit("structure role-map depth exceeded"));
        }
    }
}
pub(super) fn heading(name: &[u8]) -> bool {
    name.strip_prefix(b"H").is_some_and(|n| {
        n.first().is_some_and(|n| matches!(n, b'1'..=b'9')) && n.iter().all(u8::is_ascii_digit)
    })
}
fn standard(name: &Name, namespace: StructureNamespace) -> bool {
    let name = name.as_bytes();
    if heading(name) {
        return namespace == StructureNamespace::Pdf20
            || matches!(name, b"H1" | b"H2" | b"H3" | b"H4" | b"H5" | b"H6");
    }
    let legacy = matches!(
        name,
        b"Art"
            | b"BlockQuote"
            | b"TOC"
            | b"TOCI"
            | b"Index"
            | b"Private"
            | b"Quote"
            | b"Note"
            | b"Reference"
            | b"BibEntry"
            | b"Code"
    );
    let modern = matches!(
        name,
        b"DocumentFragment"
            | b"Aside"
            | b"Title"
            | b"FENote"
            | b"Sub"
            | b"Em"
            | b"Strong"
            | b"Artifact"
    );
    if legacy {
        return namespace == StructureNamespace::Pdf17;
    }
    if modern {
        return namespace == StructureNamespace::Pdf20;
    }
    matches!(
        name,
        b"Document"
            | b"Part"
            | b"Sect"
            | b"Div"
            | b"NonStruct"
            | b"P"
            | b"H"
            | b"L"
            | b"LI"
            | b"Lbl"
            | b"LBody"
            | b"Table"
            | b"TR"
            | b"TH"
            | b"TD"
            | b"THead"
            | b"TBody"
            | b"TFoot"
            | b"Span"
            | b"Link"
            | b"Annot"
            | b"Ruby"
            | b"RB"
            | b"RT"
            | b"RP"
            | b"Warichu"
            | b"WT"
            | b"WP"
            | b"Figure"
            | b"Formula"
            | b"Form"
            | b"Caption"
    )
}
