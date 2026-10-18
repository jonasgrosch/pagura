//! Provider oracles generated independently in tools/fixturegen/text.py.
use super::*;
use pagura_core::{
    Error, Value,
    syntax::{Origin, Parser},
};
use pagura_testkit::fixture;

fn expected(name: &str, suffix: &str) -> Result<Vec<Value>> {
    let bytes = fixture("text", &format!("{name}.{suffix}"));
    let value = Parser::new(&bytes, Origin::Buffer, Limits::default()).parse()?;
    if let Value::Array(values) = value {
        Ok(values)
    } else {
        Err(Error::syntax("oracle must be an array"))
    }
}
fn row(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .ok_or_else(|| Error::syntax("oracle must be a row"))
}
fn number(value: &Value) -> Result<f64> {
    value
        .as_number()
        .ok_or_else(|| Error::syntax("oracle must be numeric"))
}
fn integer(value: &Value) -> Result<u32> {
    value
        .as_integer()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| Error::syntax("oracle must be an unsigned integer"))
}
fn text(value: &Value) -> Option<&str> {
    value
        .as_string()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
}
fn names(inventory: &str) -> Result<Vec<String>> {
    expected(inventory, "out")?
        .iter()
        .map(|value| {
            text(&row(value)?[0])
                .map(str::to_owned)
                .ok_or_else(|| Error::syntax("oracle name must be a string"))
        })
        .collect()
}
fn near(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
}

#[cfg(feature = "standard-fonts")]
#[test]
fn every_standard_font_loads_once_with_metrics_and_outlines() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-standard-inventory")? {
        let base = fixture("text", &format!("{name}.base"));
        let resource = provider
            .standard_font(&base)?
            .ok_or_else(|| Error::syntax("missing standard font"))?;
        assert_eq!(resource.metrics.name.as_bytes(), base);
        let again = provider
            .standard_font(&base)?
            .ok_or_else(|| Error::syntax("missing cached font"))?;
        assert!(Arc::ptr_eq(&resource, &again));
        assert!(resource.program.is_some(), "{name}");
        if name == format!("standard14-{}", String::from_utf8_lossy(&base)) {
            check_afm_metrics(&name, &resource)?;
        }
    }
    Ok(())
}

#[cfg(feature = "cmaps")]
#[test]
fn every_predefined_cmap_loads_once_and_matches_its_file_oracles() -> Result<()> {
    let provider = Bundled::default();
    for name in names("bundled-cmap-inventory")? {
        let cmap_name = fixture("text", &format!("{name}.cmap"));
        let cmap = provider
            .predefined_cmap(&cmap_name)?
            .ok_or_else(|| Error::syntax("missing predefined CMap"))?;
        let again = provider
            .predefined_cmap(&cmap_name)?
            .ok_or_else(|| Error::syntax("missing cached CMap"))?;
        assert!(Arc::ptr_eq(&cmap, &again));
        let mode = fixture("text", &format!("{name}.mode"));
        assert_eq!(
            cmap.writing_mode(),
            if mode == b"1" {
                pagura_text::WritingMode::Vertical
            } else {
                pagura_text::WritingMode::Horizontal
            }
        );
        let collection = fixture("text", &format!("{name}.collection"));
        assert_eq!(
            cmap.collection(),
            Some((b"Adobe".as_slice(), collection.as_slice()))
        );
        let input = fixture("text", &format!("{name}.input"));
        let mut bytes = input.as_slice();
        for value in expected(&name, "out")? {
            let values = row(&value)?;
            let (code, len, cid) = cmap.decode_code(bytes, &Limits::default())?;
            assert_eq!(
                (code, u32::from(len), cid),
                (
                    integer(&values[0])?,
                    integer(&values[1])?,
                    integer(&values[2])?
                ),
                "{name}"
            );
            bytes = &bytes[usize::from(len)..];
        }
        assert_eq!(bytes, []);
    }
    Ok(())
}

#[cfg(feature = "cmaps")]
#[test]
fn collection_unicode_aliases_match_independent_rows() -> Result<()> {
    let provider = Bundled::default();
    for collection in ["CNS1", "GB1", "Japan1", "Korea1"] {
        let mapping = provider
            .cid_to_unicode(b"Adobe", collection.as_bytes())?
            .ok_or_else(|| Error::syntax("missing collection"))?;
        let again = provider
            .cid_to_unicode(b"Adobe", collection.as_bytes())?
            .ok_or_else(|| Error::syntax("missing cached collection"))?;
        assert!(Arc::ptr_eq(&mapping, &again));
        for value in expected(&format!("collection-{collection}"), "out")? {
            let values = row(&value)?;
            assert_eq!(mapping.get(integer(&values[0])?), text(&values[1]));
        }
    }
    Ok(())
}

#[test]
fn unavailable_resources_are_explicit_and_not_cached_as_guesses() -> Result<()> {
    let provider = Bundled::default();
    assert!(provider.cid_font(b"Unknown", b"Japan1")?.is_none());
    assert!(provider.cid_font(b"Adobe", b"Unknown")?.is_none());
    #[cfg(not(feature = "cjk-fonts"))]
    assert!(provider.cid_font(b"Adobe", b"Japan1")?.is_none());
    assert!(provider.standard_font(b"Unknown")?.is_none());
    assert!(provider.predefined_cmap(b"Unknown")?.is_none());
    assert!(provider.cid_to_unicode(b"Unknown", b"Japan1")?.is_none());
    assert!(provider.cid_to_unicode(b"Adobe", b"Unknown")?.is_none());
    #[cfg(not(feature = "standard-fonts"))]
    assert!(provider.standard_font(b"Helvetica")?.is_none());
    #[cfg(not(feature = "cmaps"))]
    assert!(provider.predefined_cmap(b"90ms-RKSJ-H")?.is_none());
    Ok(())
}

#[test]
fn parsing_budgets_fail_before_publication() {
    // Each font budget is zeroed together with the general field that the
    // pagura-text and pagura-font parsers of these files have used.
    for limits in [
        Limits {
            font_work: 0,
            operations: 0,
            ..Limits::default()
        },
        Limits {
            font_entries: 0,
            value_nodes: 0,
            ..Limits::default()
        },
        Limits {
            decoded_bytes: 0,
            ..Limits::default()
        },
    ] {
        let provider = Bundled::new(limits);
        #[cfg(feature = "cjk-fonts")]
        assert_eq!(
            provider.cid_font(b"Adobe", b"Japan1").unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
        #[cfg(feature = "standard-fonts")]
        assert_eq!(
            provider.standard_font(b"Helvetica").unwrap_err().kind,
            pagura_core::ErrorKind::Limit
        );
        #[cfg(feature = "cmaps")]
        {
            assert_eq!(
                provider.predefined_cmap(b"90ms-RKSJ-H").unwrap_err().kind,
                pagura_core::ErrorKind::Limit
            );
            assert_eq!(
                provider
                    .cid_to_unicode(b"Adobe", b"Japan1")
                    .unwrap_err()
                    .kind,
                pagura_core::ErrorKind::Limit
            );
        }
    }
    #[cfg(feature = "cmaps")]
    assert_eq!(
        Bundled::new(Limits {
            depth: 1,
            ..Limits::default()
        })
        .predefined_cmap(b"90ms-RKSJ-V")
        .unwrap_err()
        .kind,
        pagura_core::ErrorKind::Limit
    );
}

#[cfg(feature = "cmaps")]
#[test]
fn resource_inheritance_cannot_redefine_its_adopted_codespace() {
    let provider = Bundled::default();
    let error = PredefinedCMap::parse(
        b"/90ms-RKSJ-H usecmap 1 begincodespacerange <00> <ff> endcodespacerange",
        &Limits::default(),
        |name| {
            provider
                .predefined_cmap(name)?
                .ok_or_else(|| Error::syntax("missing parent"))
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, pagura_core::ErrorKind::Syntax);
}

#[cfg(feature = "standard-fonts")]
fn check_afm_metrics(name: &str, resource: &StandardFont) -> Result<()> {
    for value in expected(name, "metrics")? {
        let values = row(&value)?;
        let glyph_name = pagura_core::Name::new(
            values[1]
                .as_string()
                .ok_or_else(|| Error::syntax("missing AFM glyph name"))?,
        );
        near(
            *resource
                .metrics
                .widths
                .get(&glyph_name)
                .ok_or_else(|| Error::syntax("missing AFM glyph width"))?,
            number(&values[2])?,
        );
        if let Some(code) = values[0]
            .as_integer()
            .and_then(|code| u8::try_from(code).ok())
        {
            assert_eq!(
                resource.metrics.encoding[usize::from(code)].as_ref(),
                Some(&glyph_name)
            );
            near(
                resource
                    .metrics
                    .width(code)
                    .ok_or_else(|| Error::syntax("missing AFM code width"))?,
                number(&values[2])?,
            );
        }
    }
    let descriptor = expected(name, "descriptor")?;
    let values = row(&descriptor[0])?;
    let d = &resource.metrics.descriptor;
    for (actual, expected) in d.bbox.iter().zip(values) {
        near(*actual, number(expected)?);
    }
    near(d.italic_angle, number(&values[4])?);
    assert_eq!(Some(d.fixed_pitch), values[5].as_bool());
    for (actual, expected) in [
        d.ascent,
        d.descent,
        d.cap_height,
        d.x_height,
        d.stem_v,
        d.stem_h,
    ]
    .into_iter()
    .zip(&values[6..])
    {
        assert_eq!(actual, expected.as_number());
    }

    Ok(())
}

#[test]
fn notices_accompany_every_enabled_data_set() {
    let notices: Vec<_> = notices().collect();
    let has = |subject: &str, phrase: &str| {
        notices
            .iter()
            .any(|(name, text)| *name == subject && text.contains(phrase))
    };
    if cfg!(feature = "standard-fonts") {
        assert!(has(
            "Adobe Core 14 AFM files",
            "not distributed without this file"
        ));
        assert!(has(
            "Arimo, Tinos and Cousine",
            "Apache License, Version 2.0"
        ));
        assert!(has("Apache License, Version 2.0", "TERMS AND CONDITIONS"));
        assert!(has(
            "Foxit Symbol and Dingbats",
            "Redistributions in binary form"
        ));
    }
    if cfg!(feature = "cjk-fonts") {
        assert!(has("Source Han Sans", "SIL OPEN FONT LICENSE"));
    } else {
        assert!(!notices.iter().any(|(name, _)| *name == "Source Han Sans"));
    }
    if cfg!(feature = "cmaps") {
        assert!(has(
            "Adobe cmap-resources",
            "Redistributions in binary form"
        ));
    }
}

#[cfg(feature = "cjk-fonts")]
#[test]
fn every_collection_substitute_loads_once() -> Result<()> {
    let provider = Bundled::default();
    for ordering in ["Japan1", "GB1", "CNS1", "Korea1"] {
        let first = provider
            .cid_font(b"Adobe", ordering.as_bytes())?
            .ok_or_else(|| Error::syntax("missing CJK program"))?;
        let again = provider
            .cid_font(b"Adobe", ordering.as_bytes())?
            .ok_or_else(|| Error::syntax("missing cached CJK program"))?;
        assert!(Arc::ptr_eq(&first, &again));
    }
    Ok(())
}
