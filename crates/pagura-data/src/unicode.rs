//! The pinned cid2unicode projection: CID and comma-separated UTF-32 aliases.
use pagura_core::{Error, Limits, Meter, Position, Result, ResultExt};
use pagura_text::CidToUnicode;
use std::collections::BTreeMap;

/// Parses one collection's file. Every line is charged to `font_work` and
/// every mapped CID to `font_entries`; errors carry the offset of their line.
pub(crate) fn parse(data: &[u8], limits: &Limits) -> Result<CidToUnicode> {
    // A bundled file stands in for the decoded stream a PDF would embed.
    if data.len() > limits.decoded_bytes {
        return Err(Error::limit("CID Unicode byte budget exceeded"));
    }
    let text = std::str::from_utf8(data).map_err(|error| {
        Error::syntax("invalid CID Unicode text").at(Position::Buffer(error.valid_up_to()))
    })?;
    let mut work = Meter::new(limits.font_work, "CID Unicode work budget exceeded");
    let mut entries = Meter::new(limits.font_entries, "CID Unicode entry budget exceeded");
    let mut values = BTreeMap::new();
    for (offset, line) in crate::lines(text) {
        work.charge(1).at(Position::Buffer(offset))?;
        if line.is_empty() || line.starts_with('#') || line.starts_with("CID\t") {
            continue;
        }
        entries.charge(1).at(Position::Buffer(offset))?;
        let (cid, scalar) = row(line).at(Position::Buffer(offset))?;
        if values.insert(u32::from(cid), scalar.to_string()).is_some() {
            return Err(Error::syntax("duplicate Unicode CID").at(Position::Buffer(offset)));
        }
    }
    CidToUnicode::new(values, limits)
}

/// A `CID<TAB>aliases` row. The choice among aliases is deterministic: the
/// first horizontal value, or the first vertical (`v`) one if there is no
/// horizontal one. Aliases are alternatives, never a sequence of characters.
fn row(line: &str) -> Result<(u16, char)> {
    let (cid, aliases) = line
        .split_once('\t')
        .ok_or_else(|| Error::syntax("invalid CID Unicode row"))?;
    let cid = cid
        .parse::<u16>()
        .map_err(|_| Error::syntax("invalid Unicode CID"))?;
    let mut horizontal = None;
    let mut first = None;
    for alias in aliases.split(',') {
        let alias = alias.trim();
        let (raw, vertical) = alias
            .strip_suffix('v')
            .map_or((alias, false), |raw| (raw, true));
        if raw.len() != 8 {
            return Err(Error::syntax("invalid UTF-32 alias length"));
        }
        let scalar = u32::from_str_radix(raw, 16)
            .ok()
            .and_then(char::from_u32)
            .ok_or_else(|| Error::syntax("invalid CID Unicode scalar"))?;
        first.get_or_insert(scalar);
        if !vertical {
            horizontal.get_or_insert(scalar);
        }
    }
    let scalar = horizontal
        .or(first)
        .ok_or_else(|| Error::syntax("missing Unicode alias"))?;
    Ok((cid, scalar))
}

#[cfg(test)]
mod tests {
    use super::parse;
    use pagura_core::{ErrorKind, Limits, Position, Result};

    #[test]
    fn rows_reject_invalid_scalars_cids_and_duplicate_definitions() {
        for row in [
            "1\t0000d800",
            "65536\t00000041",
            "1\t00110000",
            "1\t00000041\n1\t00000042",
            "1\t0000004",
            "1 00000041",
        ] {
            let error = parse(row.as_bytes(), &Limits::default()).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Syntax, "{row:?}");
        }
    }

    #[test]
    fn the_first_horizontal_alias_wins_and_errors_name_their_line() -> Result<()> {
        let mapping = parse(
            b"CID\tUniJIS-UTF32\n# comment\r\n1\t00000041v,00000042\n2\t00000043v,00000044v\n",
            &Limits::default(),
        )?;
        assert_eq!(
            (mapping.get(1), mapping.get(2), mapping.get(3)),
            (Some("B"), Some("C"), None)
        );
        let error = parse(b"1\t00000041\n2\t0000d800\n", &Limits::default()).unwrap_err();
        assert_eq!(error.position, Position::Buffer(11));
        Ok(())
    }

    #[test]
    fn lines_are_font_work_and_rows_are_font_entries() {
        let data = b"# header\n1\t00000041\n";
        for (limits, message) in [
            (
                Limits {
                    font_work: 1,
                    ..Limits::default()
                },
                "CID Unicode work budget exceeded",
            ),
            (
                Limits {
                    font_entries: 0,
                    ..Limits::default()
                },
                "CID Unicode entry budget exceeded",
            ),
            (
                Limits {
                    decoded_bytes: data.len() - 1,
                    ..Limits::default()
                },
                "CID Unicode byte budget exceeded",
            ),
        ] {
            let error = parse(data, &limits).unwrap_err();
            assert_eq!((error.kind, error.message), (ErrorKind::Limit, message));
        }
        // Content-stream operations are not a font budget (ADR 0016).
        let limits = Limits {
            operations: 0,
            ..Limits::default()
        };
        assert!(parse(data, &limits).is_ok());
    }
}
