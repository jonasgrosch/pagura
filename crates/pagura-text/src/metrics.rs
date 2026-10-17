//! Simple widths and CID horizontal/vertical metrics (9.6 and 9.7.4.3).
use crate::{
    VerticalMetrics,
    cmap::charge,
    objects::{array, number, optional},
};
use pagura_core::{Dictionary, Error, Result, Value};
use pagura_document::Document;
use std::collections::BTreeMap;

#[derive(Debug)]
pub(crate) struct Metrics {
    widths: BTreeMap<u32, f64>,
    vertical: BTreeMap<u32, VerticalMetrics>,
    missing: f64,
    default_vertical: (f64, f64),
}

impl Metrics {
    pub(crate) fn simple(
        document: &Document,
        font: &Dictionary,
        descriptor: Option<&Dictionary>,
        scale: f64,
    ) -> Result<Self> {
        let missing =
            descriptor.map_or(Ok(0.0), |d| number(document, d, b"MissingWidth", 0.0))? * scale;
        let mut result = Self {
            widths: BTreeMap::new(),
            vertical: BTreeMap::new(),
            missing,
            default_vertical: (0.88, -1.0),
        };
        if let Some(value) = optional(document, font, b"Widths")? {
            let first = optional(document, font, b"FirstChar")?
                .and_then(Value::as_integer)
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(|| Error::syntax("invalid FirstChar"))?;
            let last = optional(document, font, b"LastChar")?
                .and_then(Value::as_integer)
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(|| Error::syntax("invalid LastChar"))?;
            let widths = array(value)?;
            if last < first || widths.len() != usize::from(last - first) + 1 {
                return Err(Error::syntax(
                    "Widths length disagrees with FirstChar and LastChar",
                ));
            }
            for (code, value) in (u32::from(first)..=u32::from(last)).zip(widths) {
                result.widths.insert(code, scaled(document, value, scale)?);
            }
        } else {
            return Err(Error::syntax("simple font missing Widths"));
        }
        Ok(result)
    }

    pub(crate) fn composite(document: &Document, font: &Dictionary) -> Result<Self> {
        let missing = number(document, font, b"DW", 1000.0)? / 1000.0;
        let default_vertical = if let Some(value) = optional(document, font, b"DW2")? {
            let values = array(value)?;
            if values.len() != 2 {
                return Err(Error::syntax("DW2 must contain two numbers"));
            }
            (
                scaled(document, &values[0], 0.001)?,
                scaled(document, &values[1], 0.001)?,
            )
        } else {
            (0.88, -1.0)
        };
        let mut result = Self {
            widths: BTreeMap::new(),
            vertical: BTreeMap::new(),
            missing,
            default_vertical,
        };
        let mut work = 0;
        for (key, vertical) in [(b"W".as_slice(), false), (b"W2".as_slice(), true)] {
            if let Some(value) = optional(document, font, key)? {
                result.read(document, array(value)?, vertical, &mut work)?;
            }
        }
        Ok(result)
    }

    fn read(
        &mut self,
        document: &Document,
        values: &[Value],
        vertical: bool,
        work: &mut usize,
    ) -> Result<()> {
        let mut values = values.iter();
        let stride = if vertical { 3 } else { 1 };
        while let Some(value) = values.next() {
            let first = cid(document, value)?;
            let value = document.resolve(
                values
                    .next()
                    .ok_or_else(|| Error::syntax("truncated CID metrics"))?,
            )?;
            if let Some(array) = value.as_array() {
                if array.len() % stride != 0 {
                    return Err(Error::syntax("incomplete vertical metric triple"));
                }
                for (i, chunk) in array.chunks_exact(stride).enumerate() {
                    let i =
                        u32::try_from(i).map_err(|_| Error::limit("CID metric range too large"))?;
                    let code = first
                        .checked_add(i)
                        .filter(|c| *c <= 65535)
                        .ok_or_else(|| Error::syntax("CID metrics overflow"))?;
                    charge(
                        work,
                        1,
                        document
                            .limits()
                            .operations
                            .min(document.limits().value_nodes),
                    )?;
                    self.insert(document, code, chunk, vertical)?;
                }
            } else {
                let last = cid(document, value)?;
                if last < first {
                    return Err(Error::syntax("reversed CID metric range"));
                }
                let chunk = values.by_ref().take(stride).collect::<Vec<_>>();
                if chunk.len() != stride {
                    return Err(Error::syntax("truncated CID metric range"));
                }
                let owned: Vec<_> = chunk.into_iter().cloned().collect();
                for code in first..=last {
                    charge(
                        work,
                        1,
                        document
                            .limits()
                            .operations
                            .min(document.limits().value_nodes),
                    )?;
                    self.insert(document, code, &owned, vertical)?;
                }
            }
        }
        Ok(())
    }

    fn insert(
        &mut self,
        document: &Document,
        code: u32,
        values: &[Value],
        vertical: bool,
    ) -> Result<()> {
        let first = scaled(document, &values[0], 0.001)?;
        if vertical {
            let metrics = VerticalMetrics {
                advance: first,
                origin: (
                    scaled(document, &values[1], 0.001)?,
                    scaled(document, &values[2], 0.001)?,
                ),
            };
            self.vertical.entry(code).or_insert(metrics);
        } else {
            self.widths.entry(code).or_insert(first);
        }
        Ok(())
    }

    pub(crate) fn width(&self, code: u32) -> f64 {
        self.widths.get(&code).copied().unwrap_or(self.missing)
    }
    pub(crate) fn vertical(&self, code: u32) -> VerticalMetrics {
        self.vertical
            .get(&code)
            .copied()
            .unwrap_or(VerticalMetrics {
                advance: self.default_vertical.1,
                origin: (self.width(code) / 2.0, self.default_vertical.0),
            })
    }
}

fn cid(document: &Document, value: &Value) -> Result<u32> {
    document
        .resolve(value)?
        .as_integer()
        .and_then(|v| u16::try_from(v).ok())
        .map(u32::from)
        .ok_or_else(|| Error::syntax("CID outside 16-bit range"))
}

fn scaled(document: &Document, value: &Value, scale: f64) -> Result<f64> {
    let width = document
        .resolve(value)?
        .as_number()
        .ok_or_else(|| Error::syntax("font metric is not a finite number"))?
        * scale;
    if !width.is_finite() {
        return Err(Error::syntax("font metric overflow"));
    }
    Ok(width)
}
