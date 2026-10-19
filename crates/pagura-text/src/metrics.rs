//! Simple widths and CID horizontal/vertical metrics (9.6 and 9.7.4.3).
use crate::{
    VerticalMetrics,
    cid::Cid,
    objects::{array, number, optional},
    repairs,
    report::Report,
};
use pagura_core::{Diagnostic, Dictionary, Error, Meter, Result, Value};
use pagura_document::Document;
use std::collections::BTreeMap;

/// Glyph-space units per text-space unit for every font type except Type 3
/// (9.2.4): widths and vertical metrics are in thousandths of text space.
pub(crate) const GLYPH_SPACE_UNITS: f64 = 0.001;

/// The default `/DW2` of a CIDFont, `[880 -1000]`: the vertical origin's
/// y coordinate and the vertical advance, in glyph space (Table 115).
const DEFAULT_DW2: [f64; 2] = [880.0, -1000.0];

#[derive(Debug)]
pub(crate) struct Metrics {
    horizontal: Widths,
    vertical: Runs<VerticalMetrics>,
    missing: f64,
    /// The scaled `/DW2`: vertical origin y and vertical advance.
    default_vertical: [f64; 2],
}

/// Horizontal widths: every code's width for simple fonts, CID intervals
/// for CIDFonts.
#[derive(Debug)]
enum Widths {
    Simple(Box<[f64; 256]>),
    Composite(Runs<f64>),
}

impl Metrics {
    pub(crate) fn heap_bytes(&self) -> usize {
        let horizontal = match &self.horizontal {
            Widths::Simple(values) => size_of_val(values.as_ref()),
            Widths::Composite(runs) => runs.heap_bytes(),
        };
        horizontal.saturating_add(self.vertical.heap_bytes())
    }

    /// Widths of a standard font's built-in AFM metrics for `names`.
    pub(crate) fn standard(
        metrics: &crate::StandardMetrics,
        names: &[Option<pagura_core::Name>; 256],
    ) -> Self {
        let values = names.each_ref().map(|name| {
            name.as_ref()
                .and_then(|name| metrics.widths.get(name))
                .map_or(0.0, |width| width * GLYPH_SPACE_UNITS)
        });
        Self {
            horizontal: Widths::Simple(Box::new(values)),
            vertical: Runs::default(),
            missing: 0.0,
            default_vertical: DEFAULT_DW2.map(|value| value * GLYPH_SPACE_UNITS),
        }
    }

    /// `/Widths` of a simple font from `FirstChar`, scaled by `scale`
    /// (glyph space to text space). A `Widths` array whose length disagrees
    /// with `FirstChar` and `LastChar` is a reported repair: codes past its
    /// end use `MissingWidth`, and extra entries are ignored.
    pub(crate) fn simple(
        document: &Document,
        font: &Dictionary,
        descriptor: Option<&Dictionary>,
        scale: f64,
    ) -> Result<Self> {
        let missing =
            descriptor.map_or(Ok(0.0), |d| number(document, d, b"MissingWidth", 0.0))? * scale;
        let value = optional(document, font, b"Widths")?
            .ok_or(Error::syntax("simple font missing Widths"))?;
        let first = code(document, font, b"FirstChar", "invalid FirstChar")?;
        let last = code(document, font, b"LastChar", "invalid LastChar")?;
        let widths = array(value)?;
        let count = usize::from(last.saturating_sub(first)) + 1;
        let count = if last < first || widths.len() != count {
            Report::new(document).diagnostic(Diagnostic::new(
                repairs::WIDTHS_LENGTH,
                "Widths length disagrees with FirstChar and LastChar; missing codes use MissingWidth (Table 109)",
            ))?;
            if last < first {
                0
            } else {
                count.min(widths.len())
            }
        } else {
            count
        };
        let mut values = Box::new([missing; 256]);
        for (slot, value) in values[usize::from(first)..]
            .iter_mut()
            .zip(&widths[..count])
        {
            *slot = scaled(document, value, scale)?;
        }
        Ok(Self {
            horizontal: Widths::Simple(values),
            vertical: Runs::default(),
            missing,
            default_vertical: DEFAULT_DW2.map(|value| value * GLYPH_SPACE_UNITS),
        })
    }

    /// `/DW`, `/W`, `/DW2` and `/W2` of a CIDFont (9.7.4.3).
    pub(crate) fn composite(document: &Document, font: &Dictionary) -> Result<Self> {
        let missing = number(document, font, b"DW", 1000.0)? * GLYPH_SPACE_UNITS;
        let default_vertical = if let Some(value) = optional(document, font, b"DW2")? {
            let [origin, advance] = array(value)? else {
                return Err(Error::syntax("DW2 must contain two numbers"));
            };
            [
                scaled(document, origin, GLYPH_SPACE_UNITS)?,
                scaled(document, advance, GLYPH_SPACE_UNITS)?,
            ]
        } else {
            DEFAULT_DW2.map(|value| value * GLYPH_SPACE_UNITS)
        };
        let limits = document.limits();
        let mut budget = Budget {
            work: Meter::new(limits.font_work, "CID metrics work budget exceeded"),
            entries: Meter::new(limits.font_entries, "CID metrics entry budget exceeded"),
        };
        let horizontal = match optional(document, font, b"W")? {
            Some(value) => read_runs(document, array(value)?, 1, &mut budget, |values| {
                scaled(document, &values[0], GLYPH_SPACE_UNITS)
            })?,
            None => Runs::default(),
        };
        let vertical = match optional(document, font, b"W2")? {
            Some(value) => read_runs(document, array(value)?, 3, &mut budget, |values| {
                Ok(VerticalMetrics {
                    advance: scaled(document, &values[0], GLYPH_SPACE_UNITS)?,
                    origin: (
                        scaled(document, &values[1], GLYPH_SPACE_UNITS)?,
                        scaled(document, &values[2], GLYPH_SPACE_UNITS)?,
                    ),
                })
            })?,
            None => Runs::default(),
        };
        Ok(Self {
            horizontal: Widths::Composite(horizontal),
            vertical,
            missing,
            default_vertical,
        })
    }

    /// Horizontal width in text space of a simple-font code or a CID.
    pub(crate) fn width(&self, code: u32) -> f64 {
        let width = match &self.horizontal {
            Widths::Simple(values) => usize::try_from(code)
                .ok()
                .and_then(|code| values.get(code))
                .copied(),
            Widths::Composite(runs) => Cid::try_from(code).ok().and_then(|cid| runs.get(cid)),
        };
        width.unwrap_or(self.missing)
    }

    /// Vertical metrics of a CID (9.7.4.3): `/W2`, else the `/DW2` advance
    /// with the origin at half the horizontal width.
    pub(crate) fn vertical(&self, code: u32) -> VerticalMetrics {
        Cid::try_from(code)
            .ok()
            .and_then(|cid| self.vertical.get(cid))
            .unwrap_or(VerticalMetrics {
                advance: self.default_vertical[1],
                origin: (self.width(code) / 2.0, self.default_vertical[0]),
            })
    }
}

fn code(document: &Document, font: &Dictionary, key: &[u8], message: &'static str) -> Result<u8> {
    optional(document, font, key)?
        .and_then(Value::as_integer)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or(Error::syntax(message))
}

struct Budget {
    work: Meter,
    entries: Meter,
}

/// Disjoint CID intervals of one metric. An earlier `/W` or `/W2` entry
/// wins over a later overlapping one, so only uncovered CIDs are added.
#[derive(Debug)]
struct Runs<T>(BTreeMap<Cid, Run<T>>);

#[derive(Debug)]
struct Run<T> {
    last: Cid,
    values: RunValues<T>,
}

/// `c [w1 w2 …]` gives one value per CID; `c_first c_last w` one for all.
#[derive(Debug)]
enum RunValues<T> {
    Each(Box<[T]>),
    Constant(T),
}

impl<T> Default for Runs<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<T: Copy> Runs<T> {
    fn heap_bytes(&self) -> usize {
        self.0.values().fold(
            crate::memory::tree::<Cid, Run<T>>(self.0.len()),
            |sum, run| match &run.values {
                RunValues::Each(values) => sum.saturating_add(size_of_val(values.as_ref())),
                RunValues::Constant(_) => sum,
            },
        )
    }

    fn get(&self, cid: Cid) -> Option<T> {
        let (first, run) = self.0.range(..=cid).next_back()?;
        if cid > run.last {
            return None;
        }
        Some(match &run.values {
            RunValues::Each(values) => *values.get(usize::from(cid.get() - first.get()))?,
            RunValues::Constant(value) => *value,
        })
    }

    /// Adds the CIDs of `first..=last` that no earlier entry covers.
    fn insert(
        &mut self,
        first: Cid,
        last: Cid,
        values: &RunValues<T>,
        budget: &mut Budget,
    ) -> Result<()> {
        let covered: Vec<(Cid, Cid)> = self
            .0
            .range(..=last)
            .rev()
            .take_while(|(_, run)| run.last >= first)
            .map(|(start, run)| (*start, run.last))
            .collect();
        budget.work.charge(covered.len().saturating_add(1))?;
        let mut gaps = Vec::new();
        let mut cursor = Some(first);
        for (start, end) in covered.into_iter().rev() {
            let Some(from) = cursor else { break };
            if start > from {
                gaps.push((from, Cid::new(start.get() - 1)));
            }
            cursor = end
                .checked_add(1)
                .filter(|next| *next <= last)
                .map(|next| next.max(from));
        }
        if let Some(from) = cursor {
            gaps.push((from, last));
        }
        for (from, to) in gaps {
            budget.entries.charge(1)?;
            let values = match values {
                RunValues::Each(all) => {
                    let start = usize::from(from.get() - first.get());
                    let end = usize::from(to.get() - first.get()) + 1;
                    budget.work.charge(end - start)?;
                    RunValues::Each(all[start..end].into())
                }
                RunValues::Constant(value) => RunValues::Constant(*value),
            };
            self.0.insert(from, Run { last: to, values });
        }
        Ok(())
    }
}

/// Reads `/W` (`stride` 1) or `/W2` (`stride` 3): `c [v …]` and
/// `c_first c_last v` entries (9.7.4.3). Each value group is parsed once.
fn read_runs<T: Copy>(
    document: &Document,
    entries: &[Value],
    stride: usize,
    budget: &mut Budget,
    parse: impl Fn(&[Value]) -> Result<T>,
) -> Result<Runs<T>> {
    let mut runs = Runs::default();
    let mut entries = entries.iter();
    while let Some(value) = entries.next() {
        let first = cid(document, value)?;
        let value = document.resolve(
            entries
                .next()
                .ok_or(Error::syntax("truncated CID metrics"))?,
        )?;
        if let Some(values) = value.as_array() {
            if values.len() % stride != 0 {
                return Err(Error::syntax("incomplete vertical metric triple"));
            }
            let count = values.len() / stride;
            let Some(last) = count
                .checked_sub(1)
                .and_then(|offset| u32::try_from(offset).ok())
                .map(|offset| first.checked_add(offset))
            else {
                continue;
            };
            let last = last.ok_or(Error::syntax("CID metrics overflow"))?;
            budget.work.charge(count)?;
            let parsed = values
                .chunks_exact(stride)
                .map(&parse)
                .collect::<Result<Box<[T]>>>()?;
            runs.insert(first, last, &RunValues::Each(parsed), budget)?;
        } else {
            let last = cid(document, value)?;
            if last < first {
                return Err(Error::syntax("reversed CID metric range"));
            }
            let group: Vec<&Value> = entries.by_ref().take(stride).collect();
            if group.len() != stride {
                return Err(Error::syntax("truncated CID metric range"));
            }
            let group: Vec<Value> = group
                .into_iter()
                .map(|value| document.resolve(value).cloned())
                .collect::<Result<_>>()?;
            budget.work.charge(stride)?;
            runs.insert(first, last, &RunValues::Constant(parse(&group)?), budget)?;
        }
    }
    Ok(runs)
}

fn cid(document: &Document, value: &Value) -> Result<Cid> {
    document
        .resolve(value)?
        .as_integer()
        .and_then(Cid::from_integer)
        .ok_or(Error::syntax("CID outside 16-bit range"))
}

fn scaled(document: &Document, value: &Value, scale: f64) -> Result<f64> {
    let width = document
        .resolve(value)?
        .as_number()
        .ok_or(Error::syntax("font metric is not a finite number"))?
        * scale;
    if !width.is_finite() {
        return Err(Error::syntax("font metric overflow"));
    }
    Ok(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::{dictionary, document, expected, float, integer, near, rows};
    use pagura_core::ErrorKind;
    use pagura_core::Limits;

    fn budget() -> Budget {
        let limits = Limits::default();
        Budget {
            work: Meter::new(limits.font_work, "test"),
            entries: Meter::new(limits.font_entries, "test"),
        }
    }

    #[test]
    fn earlier_entries_win_and_gaps_are_filled() -> Result<()> {
        let mut runs = Runs::default();
        let mut budget = budget();
        runs.insert(
            Cid::new(10),
            Cid::new(12),
            &RunValues::Each([1.0, 2.0, 3.0].into()),
            &mut budget,
        )?;
        runs.insert(
            Cid::new(20),
            Cid::new(20),
            &RunValues::Constant(4.0),
            &mut budget,
        )?;
        runs.insert(
            Cid::new(5),
            Cid::new(25),
            &RunValues::Constant(9.0),
            &mut budget,
        )?;
        let values: Vec<Option<f64>> = [4, 5, 9, 10, 12, 13, 19, 20, 21, 25, 26]
            .map(|cid| runs.get(Cid::new(cid)))
            .into();
        assert_eq!(
            values,
            [
                None,
                Some(9.0),
                Some(9.0),
                Some(1.0),
                Some(3.0),
                Some(9.0),
                Some(9.0),
                Some(4.0),
                Some(9.0),
                Some(9.0),
                None
            ]
        );
        runs.insert(
            Cid::new(65_530),
            Cid::new(65_535),
            &RunValues::Each([0.5; 6].into()),
            &mut budget,
        )?;
        assert_eq!(runs.get(Cid::new(65_535)), Some(0.5));
        Ok(())
    }

    #[test]
    fn a_full_range_is_one_entry() -> Result<()> {
        let mut runs = Runs::default();
        let mut budget = budget();
        runs.insert(
            Cid::new(0),
            Cid::new(65_535),
            &RunValues::Constant(1.0),
            &mut budget,
        )?;
        assert_eq!(runs.0.len(), 1);
        assert_eq!(budget.entries.used(), 1);
        Ok(())
    }
    #[test]
    fn cid_metric_vectors() -> Result<()> {
        for name in ["cid-metrics", "cid-default-metrics"] {
            let document = document(name)?;
            let metrics = Metrics::composite(&document, dictionary(&document)?)?;
            for value in expected(name)? {
                let row = rows(&value)?;
                let code = integer(&row[0])?;
                near(metrics.width(code), float(&row[1])?);
                let vertical = metrics.vertical(code);
                near(vertical.advance, float(&row[2])?);
                near(vertical.origin.0, float(&row[3])?);
                near(vertical.origin.1, float(&row[4])?);
            }
        }
        Ok(())
    }

    #[test]
    fn invalid_metric_vectors() -> Result<()> {
        for name in [
            "cid-width-reversed",
            "cid-width-overflow",
            "cid-w2-truncated",
            "cid-range-truncated",
            "cid-dw2-length",
        ] {
            let document = document(name)?;
            let result = Metrics::composite(&document, dictionary(&document)?);
            assert_eq!(
                result.err().map(|e| e.kind),
                Some(ErrorKind::Syntax),
                "{name}"
            );
        }
        Ok(())
    }

    #[test]
    fn missing_width_vector() -> Result<()> {
        let document = document("simple-missing-width")?;
        let dictionary = dictionary(&document)?;
        let descriptor = dictionary
            .get(b"FontDescriptor")
            .and_then(Value::as_dictionary);
        let metrics = Metrics::simple(&document, dictionary, descriptor, 0.001)?;
        for value in expected("simple-missing-width")? {
            let row = rows(&value)?;
            near(metrics.width(integer(&row[0])?), float(&row[1])?);
        }
        Ok(())
    }
}
