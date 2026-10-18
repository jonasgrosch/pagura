//! The standard 14 fonts (9.6.2.2): pinned Core14 AFM metrics and
//! metric-compatible substitute outlines (ADR 0009).
use crate::{
    afm,
    cache::Cache,
    entry::{File, Program},
};
use pagura_core::{Limits, Result};
#[cfg(feature = "standard-outlines")]
use pagura_font::FontFormat;
use pagura_text::StandardFont;
use std::sync::Arc;

/// The standard font `name` (a canonical base name): its metrics and, if
/// linked, its substitute outlines. `None` if no metrics are linked for it.
pub(crate) fn standard_font(
    name: &[u8],
    cache: &Cache<StandardFont>,
    limits: &Limits,
) -> Result<Option<Arc<StandardFont>>> {
    let Some(metrics) = METRICS.iter().find(|file| file.key == name) else {
        return Ok(None);
    };
    let outline = OUTLINES.iter().find(|program| program.key == name);
    cache
        .get_or_try_insert(metrics.key, || {
            Ok(StandardFont {
                metrics: Arc::new(afm::parse(metrics.data, limits)?),
                program: outline
                    .map(|program| program.parse(limits).map(Arc::new))
                    .transpose()?,
            })
        })
        .map(Some)
}

/// Adobe's Core14 AFM files, which supply widths and descriptors.
#[cfg(feature = "standard-metrics")]
pub(crate) const METRICS: &[File] = &[
    File {
        key: b"Courier",
        data: include_bytes!("../../../data/afm/Courier.afm"),
    },
    File {
        key: b"Courier-Bold",
        data: include_bytes!("../../../data/afm/Courier-Bold.afm"),
    },
    File {
        key: b"Courier-Oblique",
        data: include_bytes!("../../../data/afm/Courier-Oblique.afm"),
    },
    File {
        key: b"Courier-BoldOblique",
        data: include_bytes!("../../../data/afm/Courier-BoldOblique.afm"),
    },
    File {
        key: b"Helvetica",
        data: include_bytes!("../../../data/afm/Helvetica.afm"),
    },
    File {
        key: b"Helvetica-Bold",
        data: include_bytes!("../../../data/afm/Helvetica-Bold.afm"),
    },
    File {
        key: b"Helvetica-Oblique",
        data: include_bytes!("../../../data/afm/Helvetica-Oblique.afm"),
    },
    File {
        key: b"Helvetica-BoldOblique",
        data: include_bytes!("../../../data/afm/Helvetica-BoldOblique.afm"),
    },
    File {
        key: b"Times-Roman",
        data: include_bytes!("../../../data/afm/Times-Roman.afm"),
    },
    File {
        key: b"Times-Bold",
        data: include_bytes!("../../../data/afm/Times-Bold.afm"),
    },
    File {
        key: b"Times-Italic",
        data: include_bytes!("../../../data/afm/Times-Italic.afm"),
    },
    File {
        key: b"Times-BoldItalic",
        data: include_bytes!("../../../data/afm/Times-BoldItalic.afm"),
    },
    File {
        key: b"Symbol",
        data: include_bytes!("../../../data/afm/Symbol.afm"),
    },
    File {
        key: b"ZapfDingbats",
        data: include_bytes!("../../../data/afm/ZapfDingbats.afm"),
    },
];
#[cfg(not(feature = "standard-metrics"))]
pub(crate) const METRICS: &[File] = &[];

/// Arimo, Tinos and Cousine for the Latin fonts, FoxitSymbol and
/// FoxitDingbats for the symbolic ones; the AFM widths still apply.
#[cfg(feature = "standard-outlines")]
pub(crate) const OUTLINES: &[Program] = &[
    Program {
        key: b"Courier",
        data: include_bytes!("../../../data/fonts/Cousine-Regular.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Courier-Bold",
        data: include_bytes!("../../../data/fonts/Cousine-Bold.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Courier-Oblique",
        data: include_bytes!("../../../data/fonts/Cousine-Italic.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Courier-BoldOblique",
        data: include_bytes!("../../../data/fonts/Cousine-BoldItalic.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Helvetica",
        data: include_bytes!("../../../data/fonts/Arimo-Regular.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Helvetica-Bold",
        data: include_bytes!("../../../data/fonts/Arimo-Bold.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Helvetica-Oblique",
        data: include_bytes!("../../../data/fonts/Arimo-Italic.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Helvetica-BoldOblique",
        data: include_bytes!("../../../data/fonts/Arimo-BoldItalic.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Times-Roman",
        data: include_bytes!("../../../data/fonts/Tinos-Regular.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Times-Bold",
        data: include_bytes!("../../../data/fonts/Tinos-Bold.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Times-Italic",
        data: include_bytes!("../../../data/fonts/Tinos-Italic.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Times-BoldItalic",
        data: include_bytes!("../../../data/fonts/Tinos-BoldItalic.ttf"),
        format: FontFormat::TrueType,
    },
    Program {
        key: b"Symbol",
        data: include_bytes!("../../../data/fonts/FoxitSymbol.cff"),
        format: FontFormat::Cff,
    },
    Program {
        key: b"ZapfDingbats",
        data: include_bytes!("../../../data/fonts/FoxitDingbats.cff"),
        format: FontFormat::Cff,
    },
];
#[cfg(not(feature = "standard-outlines"))]
pub(crate) const OUTLINES: &[Program] = &[];

#[cfg(all(test, feature = "standard-metrics"))]
mod tests {
    use crate::{
        Bundled,
        oracle::{names, row, rows},
    };
    use pagura_core::{Error, Name, Result, Value};
    use pagura_testkit::fixture;
    use pagura_text::{Provider, StandardFont};
    use std::sync::Arc;

    fn number(value: &Value) -> Result<f64> {
        value
            .as_number()
            .ok_or_else(|| Error::syntax("oracle must be numeric"))
    }

    fn near(actual: f64, expected: &Value) -> Result<()> {
        let expected = number(expected)?;
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
        Ok(())
    }

    #[test]
    fn every_standard_font_loads_once_with_metrics_and_linked_outlines() -> Result<()> {
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
            assert_eq!(
                resource.program.is_some(),
                cfg!(feature = "standard-outlines"),
                "{name}"
            );
            if name == format!("standard14-{}", String::from_utf8_lossy(&base)) {
                check_afm_metrics(&name, &resource)?;
            }
        }
        Ok(())
    }

    fn check_afm_metrics(name: &str, resource: &StandardFont) -> Result<()> {
        for value in rows(name, "metrics")? {
            let values = row(&value)?;
            let glyph_name = Name::new(
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
                &values[2],
            )?;
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
                    &values[2],
                )?;
            }
        }
        let descriptor = rows(name, "descriptor")?;
        let values = row(&descriptor[0])?;
        let d = &resource.metrics.descriptor;
        for (actual, expected) in d.bbox.iter().zip(values) {
            near(*actual, expected)?;
        }
        near(d.italic_angle, &values[4])?;
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
}
