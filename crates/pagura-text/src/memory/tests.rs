use crate::*;
use pagura_core::{Dictionary, Error, Limits, ObjectId, Result};
use pagura_core::{Name, Value};
use pagura_document::Document;
use pagura_document::OpenOptions;
use pagura_font::FontProgram;
use pagura_testkit::fixture;
use std::collections::BTreeMap;
use std::sync::Arc;

fn load(name: &str) -> Result<(Document, Dictionary)> {
    let document = Document::open(
        fixture("text", &format!("{name}.pdf")),
        &OpenOptions::default(),
    )?;
    let dictionary = document
        .object(ObjectId::new(4, 0))?
        .dictionary()
        .ok_or_else(|| Error::syntax("missing font"))?
        .clone();
    Ok((document, dictionary))
}

#[test]
fn loaded_kinds_include_owned_tables_and_programs() -> Result<()> {
    for name in [
        "tt-unicode-priority",
        "embedded-cff",
        "embedded-type1",
        "type3-differences",
        "embedded-cid-variable-codes",
    ] {
        let (document, dict) = load(name)?;
        let font = PdfFont::load(&document, &dict)?;
        assert!(font.retained_bytes() > size_of::<PdfFont>(), "{name}");
        let before = font.retained_bytes();
        font.decode(b"A").ok();
        assert_eq!(font.retained_bytes(), before, "{name}");
        if let font::Source::Program { program, .. } = &font.source {
            assert!(before > program.data().len());
        }
    }
    Ok(())
}

#[test]
fn owned_unicode_and_type3_resources_increase_the_estimate() -> Result<()> {
    let (document, mut dict) = load("type3-differences")?;
    let base = PdfFont::load(&document, &dict)?.retained_bytes();
    let mut resources = document
        .resolve_dictionary(
            dict.get(b"Resources")
                .ok_or_else(|| Error::syntax("missing Resources"))?,
        )?
        .clone();
    resources.insert(Name::new(b"Payload"), Value::String(vec![b'x'; 32768]));
    dict.insert(Name::new(b"Resources"), Value::Dictionary(resources));
    assert!(PdfFont::load(&document, &dict)?.retained_bytes() >= base + 32768);
    let (document, dict) = load("tt-unicode-priority")?;
    let mut font = PdfFont::load(&document, &dict)?;
    let base = font.retained_bytes();
    font.to_unicode = Some(Arc::new(cmap::CMap::parse(
        b"1 begincodespacerange <00> <ff> endcodespacerange 1 beginbfchar <41> <0041> endbfchar",
        pagura_core::syntax::Origin::Buffer,
        &document.limits(),
        cmap::CMapRole::ToUnicode,
    )?));
    assert!(font.accounted().retained_bytes() > base);
    Ok(())
}

struct Data {
    standard: Arc<StandardFont>,
    cmap: PredefinedCMap,
    unicode: Arc<CidToUnicode>,
}

impl Provider for Data {
    fn standard_font(&self, _: &[u8]) -> Result<Option<Arc<StandardFont>>> {
        Ok(Some(Arc::clone(&self.standard)))
    }
    fn cid_font(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<FontProgram>>> {
        Ok(None)
    }
    fn predefined_cmap(&self, _: &[u8]) -> Result<Option<PredefinedCMap>> {
        Ok(Some(self.cmap.clone()))
    }
    fn cid_to_unicode(&self, _: &[u8], _: &[u8]) -> Result<Option<Arc<CidToUnicode>>> {
        Ok(Some(Arc::clone(&self.unicode)))
    }
}

fn provider(program: Arc<FontProgram>) -> Result<Data> {
    let limits = Limits::default();
    let mut metrics = StandardMetrics::new(Name::new(b"Helvetica"));
    metrics.encoding[65] = Some(Name::new(b"A"));
    metrics.widths.insert(Name::new(b"A"), 600.0);
    let cmap = b"/CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) >> def \
        1 begincodespacerange <0000> <ffff> endcodespacerange \
        1 begincidrange <0000> <ffff> 0 endcidrange";
    let unicode = [(1, "A".to_owned())].into_iter().collect();
    Ok(Data {
        standard: Arc::new(StandardFont::new(Arc::new(metrics), Some(program))),
        cmap: PredefinedCMap::parse(cmap, &limits, |_| Err(Error::syntax("unexpected parent")))?,
        unicode: Arc::new(CidToUnicode::new(unicode, &limits)?),
    })
}

#[test]
fn substituted_programs_and_predefined_maps_belong_to_the_provider() -> Result<()> {
    let (document, dict) = load("tt-unicode-priority")?;
    let embedded = PdfFont::load(&document, &dict)?;
    let font::Source::Program { program, .. } = &embedded.source else {
        return Err(Error::syntax("expected program"));
    };
    let mut data = provider(Arc::clone(program))?;
    let dict: Dictionary = [
        (Name::new(b"Subtype"), Value::name(b"Type1")),
        (Name::new(b"BaseFont"), Value::name(b"Helvetica")),
    ]
    .into_iter()
    .collect();
    let first = PdfFont::load_with_provider(&document, &dict, Some(&data))?;
    let second = PdfFont::load_with_provider(&document, &dict, Some(&data))?;
    assert_eq!(first.retained_bytes(), second.retained_bytes());
    assert!(first.retained_bytes() < embedded.retained_bytes());
    let (document, mut dict) = load("embedded-cid-variable-codes")?;
    let owned = PdfFont::load(&document, &dict)?.retained_bytes();
    dict.insert(Name::new(b"Encoding"), Value::name(b"Cached"));
    let first = PdfFont::load_with_provider(&document, &dict, Some(&data))?;
    assert!(owned > first.retained_bytes());
    let mut entries = BTreeMap::new();
    for cid in 0..1000 {
        entries.insert(cid, "long collection text".repeat(10));
    }
    data.unicode = Arc::new(CidToUnicode::new(entries, &Limits::default())?);
    let second = PdfFont::load_with_provider(&document, &dict, Some(&data))?;
    assert_eq!(first.retained_bytes(), second.retained_bytes());
    let font::Mapping::Composite {
        map: first,
        shared: true,
    } = first.mapping
    else {
        return Err(Error::syntax("expected shared CMap"));
    };
    let font::Mapping::Composite {
        map: second,
        shared: true,
    } = second.mapping
    else {
        return Err(Error::syntax("expected shared CMap"));
    };
    assert!(Arc::ptr_eq(&first, &second));
    Ok(())
}

#[test]
fn arithmetic_saturates() {
    assert_eq!(super::tree::<u32, f64>(usize::MAX), usize::MAX);
}

#[test]
fn cid_to_gid_maps_are_owned_storage() -> Result<()> {
    let (document, dict) = load("embedded-cid-variable-codes")?;
    let mut font = PdfFont::load(&document, &dict)?;
    let before = font.retained_bytes();
    let font::Source::Program { cid_map, .. } = &mut font.source else {
        return Err(Error::syntax("expected program"));
    };
    *cid_map = Some(cid::CidToGidMap::new(vec![0; 128].into())?);
    assert_eq!(font.accounted().retained_bytes(), before + 128);
    Ok(())
}

#[test]
fn embedded_program_storage_uses_the_programs_retained_accounting() -> Result<()> {
    for name in [
        "tt-unicode-priority",
        "embedded-cff",
        "embedded-type1",
        "embedded-cid-variable-codes",
    ] {
        let (document, dictionary) = load(name)?;
        let font = PdfFont::load(&document, &dictionary)?;
        let font::Source::Program {
            program, cid_map, ..
        } = &font.source
        else {
            return Err(Error::syntax("expected program"));
        };
        assert_eq!(
            super::source(&font.source),
            program
                .retained_bytes()
                .saturating_add(size_of_val(program.as_ref()))
                .saturating_add(2 * size_of::<usize>())
                .saturating_add(cid_map.as_ref().map_or(0, cid::CidToGidMap::heap_bytes))
        );
    }
    Ok(())
}
