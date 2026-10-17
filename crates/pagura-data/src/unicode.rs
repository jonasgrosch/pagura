//! The pinned cid2unicode projection: CID and comma-separated UTF-32 aliases.
use pagura_core::{Error, Limits, Result};
use pagura_text::CidToUnicode;
use std::collections::BTreeMap;

pub(crate) fn parse(data: &[u8], limits: Limits) -> Result<CidToUnicode> {
    if data.len() > limits.decoded_bytes {
        return Err(Error::limit("CID Unicode byte budget exceeded"));
    }
    let text = std::str::from_utf8(data).map_err(|_| Error::syntax("invalid CID Unicode text"))?;
    let mut values = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if index >= limits.operations {
            return Err(Error::limit("CID Unicode operation budget exceeded"));
        }
        if line.is_empty() || line.starts_with('#') || line.starts_with("CID\t") {
            continue;
        }
        if values.len() >= limits.value_nodes {
            return Err(Error::limit("CID Unicode entry budget exceeded"));
        }
        let (cid, aliases) = line
            .split_once('\t')
            .ok_or_else(|| Error::syntax("invalid CID Unicode row"))?;
        let cid = cid
            .parse::<u16>()
            .map_err(|_| Error::syntax("invalid Unicode CID"))?;
        // Alias choice is deterministic: first horizontal value, or first
        // vertical value if there is no horizontal one. Aliases are alternatives,
        // never a sequence of multiple Unicode characters.
        let mut selected = None;
        let mut first = None;
        for alias in aliases.split(',') {
            let alias = alias.trim();
            let raw = alias.strip_suffix('v').unwrap_or(alias);
            if raw.len() != 8 {
                return Err(Error::syntax("invalid UTF-32 alias length"));
            }
            let scalar = u32::from_str_radix(raw, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| Error::syntax("invalid CID Unicode scalar"))?;
            first.get_or_insert(scalar);
            if !alias.ends_with('v') {
                selected.get_or_insert(scalar);
            }
        }
        let scalar = selected
            .or(first)
            .ok_or_else(|| Error::syntax("missing Unicode alias"))?;
        if values.insert(u32::from(cid), scalar.to_string()).is_some() {
            return Err(Error::syntax("duplicate Unicode CID"));
        }
    }
    CidToUnicode::new(values, &limits)
}
