//! Page labels (12.4.2, Table 161), including repeated-letter numbering.
use crate::Document;
use crate::view::{Context, Source, text};
use pagura_core::{Error, ErrorKind, Result};

/// The numbering convention of a page-label range.
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
/// One page-label range; its end is the next range or the document end.
#[derive(Debug, Clone, PartialEq, Eq)]
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
pub(super) fn ranges(doc: &Document) -> Result<Vec<PageLabelRange>> {
    let mut ctx = Context::new(doc);
    let (catalog, tree_source) = ctx.catalog()?;
    let Some(tree) = ctx.present(catalog, b"PageLabels", tree_source)? else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    let pages = doc.page_count()?;
    for entry in doc
        .number_tree(tree)
        .with_source(tree_source)
        .entries_with(&mut ctx)?
    {
        let (dictionary, source) = ctx.dictionary(entry.value, entry.source)?;
        ctx.type_name(dictionary, b"PageLabel", source)?;
        let page = usize::try_from(entry.key)
            .ok()
            .filter(|n| *n < pages)
            .ok_or_else(|| {
                source.error(Error::syntax("page label range index outside document"))
            })?;
        let style = ctx
            .entry(dictionary, b"S", source)?
            .map(|v| match v.as_name().map(pagura_core::Name::as_bytes) {
                Some(b"D") => Ok(LabelStyle::Decimal),
                Some(b"R") => Ok(LabelStyle::UpperRoman),
                Some(b"r") => Ok(LabelStyle::LowerRoman),
                Some(b"A") => Ok(LabelStyle::UpperLetters),
                Some(b"a") => Ok(LabelStyle::LowerLetters),
                _ => Err(source.error(Error::syntax("unknown page label style"))),
            })
            .transpose()?;
        let prefix = ctx
            .entry(dictionary, b"P", source)?
            .map(|v| {
                v.as_string()
                    .ok_or_else(|| source.error(Error::syntax("page label prefix must be text")))
                    .and_then(|b| text::decode(&mut ctx, b, source))
            })
            .transpose()?
            .unwrap_or_default();
        let start = ctx
            .entry(dictionary, b"St", source)?
            .map(|v| {
                v.as_integer()
                    .and_then(|n| u64::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| source.error(Error::syntax("page label St must be positive")))
            })
            .transpose()?
            .unwrap_or(1);
        ctx.allocate::<PageLabelRange>(1, source)?;
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
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "page label index outside document",
            ));
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
pub(super) fn read(doc: &Document) -> Result<PageLabels> {
    Ok(PageLabels {
        ranges: ranges(doc)?,
        pages: doc.page_count()?,
        maximum: doc.limits().string_bytes.min(doc.limits().document_bytes),
    })
}
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
            for (number, upper) in [
                (1000, "M"),
                (900, "CM"),
                (500, "D"),
                (400, "CD"),
                (100, "C"),
                (90, "XC"),
                (50, "L"),
                (40, "XL"),
                (10, "X"),
                (9, "IX"),
                (5, "V"),
                (4, "IV"),
                (1, "I"),
            ] {
                let count = usize::try_from(remaining / number)
                    .map_err(|_| Error::limit("page label too long"))?;
                let size = upper
                    .len()
                    .checked_mul(count)
                    .and_then(|n| n.checked_add(result.len()))
                    .filter(|n| *n <= maximum)
                    .ok_or(Error::limit("page label too long"))?;
                result.reserve(size - result.len());
                let lower = upper.to_ascii_lowercase();
                let text = if range.style == Some(LabelStyle::LowerRoman) {
                    lower.as_str()
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
