//! Page labels (12.4.2, Table 161), including repeated-letter numbering.
use crate::Document;
use crate::source::ReadResult;
use crate::view::{Context, Source};
use pagura_core::{Error, Name, Result};

/// The numbering convention of a page-label range; Table 161 defines
/// exactly these five.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// Decimal Arabic digits.
    Decimal,
    /// Uppercase Roman numerals.
    UpperRoman,
    /// Lowercase Roman numerals.
    LowerRoman,
    /// A..Z, AA..ZZ, AAA..ZZZ, etc.; this is not spreadsheet numbering.
    UpperLetters,
    /// Lowercase repeated letters.
    LowerLetters,
}

impl TryFrom<&Name> for LabelStyle {
    type Error = Error;

    /// The `S` entry of a page-label dictionary (Table 161).
    fn try_from(name: &Name) -> Result<Self> {
        match name.as_bytes() {
            b"D" => Ok(Self::Decimal),
            b"R" => Ok(Self::UpperRoman),
            b"r" => Ok(Self::LowerRoman),
            b"A" => Ok(Self::UpperLetters),
            b"a" => Ok(Self::LowerLetters),
            _ => Err(Error::syntax("unknown page label style")),
        }
    }
}

/// One page-label range; its end is the next range or the document end.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PageLabelRange {
    source: Source,
    /// Zero-based first page.
    pub page: usize,
    /// Prefix decoded as a PDF text string.
    pub prefix: String,
    /// None produces only the prefix, without a numeric suffix.
    pub style: Option<LabelStyle>,
    /// First numeric value, at least one.
    pub start: u64,
}

pub(super) fn ranges(document: &Document) -> ReadResult<Vec<PageLabelRange>> {
    let mut context = Context::new(document);
    let (catalog, tree_source) = context.catalog()?;
    let Some(tree) = context.present(catalog, b"PageLabels", tree_source)? else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    let pages = document.page_count()?;
    for entry in document
        .number_tree(tree)
        .with_source(tree_source)
        .entries_with(&mut context)?
    {
        let (dictionary, source) = context.dictionary(entry.value, entry.source)?;
        context.type_name(dictionary, b"PageLabel", source)?;
        let page = usize::try_from(entry.key)
            .ok()
            .filter(|n| *n < pages)
            .ok_or_else(|| {
                source.error(Error::syntax("page label range index outside document"))
            })?;
        let style = context
            .entry(dictionary, b"S", source)?
            .map(|value| {
                value
                    .as_name()
                    .ok_or(Error::syntax("unknown page label style"))
                    .and_then(LabelStyle::try_from)
                    .map_err(|error| source.fault(error))
            })
            .transpose()?;
        let prefix = context
            .entry(dictionary, b"P", source)?
            .map(|v| {
                v.as_string()
                    .ok_or_else(|| source.error(Error::syntax("page label prefix must be text")))
                    .and_then(|b| context.text(b, source))
            })
            .transpose()?
            .unwrap_or_default();
        let start = context
            .entry(dictionary, b"St", source)?
            .map(|v| {
                v.as_integer()
                    .and_then(|n| u64::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| source.error(Error::syntax("page label St must be positive")))
            })
            .transpose()?
            .unwrap_or(1);
        context.allocate::<PageLabelRange>(1, source)?;
        result.push(PageLabelRange {
            source,
            page,
            prefix,
            style,
            start,
        });
    }
    if result.first().is_none_or(|r| r.page != 0) {
        return Err(tree_source.error(Error::syntax("page labels require a range at page zero")));
    }
    Ok(result)
}

/// Parsed page-label ranges, reusable for a viewer's page list. No document
/// cache is added. Formatting one label allocates only its bounded output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLabels {
    ranges: Vec<PageLabelRange>,
    pages: usize,
    maximum: usize,
}

impl PageLabels {
    /// Validated ranges, in ascending starting-page order.
    pub fn ranges(&self) -> &[PageLabelRange] {
        &self.ranges
    }

    /// Formats one page label with logarithmic range lookup.
    ///
    /// # Errors
    /// Returns InvalidArgument for an invalid page index or Limit if the label
    /// would exceed the string/result budget. Errors retain range source context.
    pub fn label(&self, index: usize) -> Result<String> {
        if index >= self.pages {
            return Err(Error::invalid_argument("page label index outside document"));
        }
        let end = self.ranges.partition_point(|range| range.page <= index);
        self.format(index, end.checked_sub(1).and_then(|n| self.ranges.get(n)))
    }

    /// Formats labels in page order, advancing through each range once. Each
    /// item can fail with a string-budget error; no strings are cached. Retaining
    /// or collecting the returned strings is the caller's responsibility.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Result<String>> + '_ {
        let mut range = 0;
        (0..self.pages).map(move |index| {
            while self
                .ranges
                .get(range + 1)
                .is_some_and(|next| next.page <= index)
            {
                range += 1;
            }
            self.format(index, self.ranges.get(range))
        })
    }

    fn format(&self, index: usize, range: Option<&PageLabelRange>) -> Result<String> {
        if let Some(range) = range {
            return format_range(range, index, self.maximum).map_err(|e| range.source.error(e));
        }
        let label = (index + 1).to_string();
        if label.len() > self.maximum {
            return Err(Error::limit("page label too long"));
        }
        Ok(label)
    }
}

pub(super) fn read(document: &Document) -> ReadResult<PageLabels> {
    Ok(PageLabels {
        ranges: ranges(document)?,
        pages: document.page_count()?,
        maximum: document
            .limits()
            .string_bytes
            .min(document.limits().document_bytes),
    })
}

/// Roman numeral values with their upper- and lowercase spellings, largest
/// first; larger values repeat M (12.4.2).
const ROMAN: [(u64, &str, &str); 13] = [
    (1000, "M", "m"),
    (900, "CM", "cm"),
    (500, "D", "d"),
    (400, "CD", "cd"),
    (100, "C", "c"),
    (90, "XC", "xc"),
    (50, "L", "l"),
    (40, "XL", "xl"),
    (10, "X", "x"),
    (9, "IX", "ix"),
    (5, "V", "v"),
    (4, "IV", "iv"),
    (1, "I", "i"),
];

fn format_range(range: &PageLabelRange, index: usize, maximum: usize) -> Result<String> {
    let offset = u64::try_from(index - range.page)
        .map_err(|_| Error::limit("page label number overflow"))?;
    let value = range
        .start
        .checked_add(offset)
        .ok_or(Error::limit("page label number overflow"))?;
    let mut result = range.prefix.clone();
    match range.style {
        None => {}
        Some(LabelStyle::Decimal) => append(&mut result, &value.to_string(), maximum)?,
        Some(LabelStyle::UpperRoman | LabelStyle::LowerRoman) => {
            let mut remaining = value;
            for (number, upper, lower) in ROMAN {
                let count = usize::try_from(remaining / number)
                    .map_err(|_| Error::limit("page label too long"))?;
                let size = upper
                    .len()
                    .checked_mul(count)
                    .and_then(|n| n.checked_add(result.len()))
                    .filter(|n| *n <= maximum)
                    .ok_or(Error::limit("page label too long"))?;
                result.reserve(size - result.len());
                let text = if range.style == Some(LabelStyle::LowerRoman) {
                    lower
                } else {
                    upper
                };
                for _ in 0..count {
                    result.push_str(text);
                }
                remaining %= number;
            }
        }
        Some(LabelStyle::UpperLetters | LabelStyle::LowerLetters) => {
            let count = usize::try_from((value - 1) / 26 + 1)
                .map_err(|_| Error::limit("page label too long"))?;
            if result.len().checked_add(count).is_none_or(|n| n > maximum) {
                return Err(Error::limit("page label too long"));
            }
            let base = if range.style == Some(LabelStyle::LowerLetters) {
                b'a'
            } else {
                b'A'
            };
            let letter = char::from(
                base + u8::try_from((value - 1) % 26)
                    .map_err(|_| Error::limit("page label letter overflow"))?,
            );
            result.extend(std::iter::repeat_n(letter, count));
        }
    }
    Ok(result)
}

fn append(out: &mut String, text: &str, maximum: usize) -> Result<()> {
    if out
        .len()
        .checked_add(text.len())
        .is_none_or(|n| n > maximum)
    {
        return Err(Error::limit("page label too long"));
    }
    out.push_str(text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LabelStyle, PageLabelRange, format_range};
    use crate::view::Source;

    fn label(style: LabelStyle, value: u64) -> String {
        let range = PageLabelRange {
            source: Source::default(),
            page: 0,
            prefix: "p-".into(),
            style: Some(style),
            start: value,
        };
        format_range(&range, 0, 1024).unwrap()
    }

    #[test]
    fn numerals_follow_table_161() {
        for (value, expected) in [
            (1, "p-I"),
            (4, "p-IV"),
            (14, "p-XIV"),
            (499, "p-CDXCIX"),
            (500, "p-D"),
            (501, "p-DI"),
            (899, "p-DCCCXCIX"),
            (3999, "p-MMMCMXCIX"),
            (4000, "p-MMMM"),
        ] {
            assert_eq!(label(LabelStyle::UpperRoman, value), expected);
            assert_eq!(
                label(LabelStyle::LowerRoman, value),
                format!("p-{}", expected[2..].to_ascii_lowercase())
            );
        }
        // A to Z, then AA to ZZ, then AAA: letters repeat, they do not carry.
        for (value, expected) in [
            (1, "p-A"),
            (26, "p-Z"),
            (27, "p-AA"),
            (28, "p-BB"),
            (53, "p-AAA"),
        ] {
            assert_eq!(label(LabelStyle::UpperLetters, value), expected);
            assert_eq!(
                label(LabelStyle::LowerLetters, value),
                format!("p-{}", expected[2..].to_ascii_lowercase())
            );
        }
        assert_eq!(label(LabelStyle::Decimal, 12), "p-12");
    }
}
