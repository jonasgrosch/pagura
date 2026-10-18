//! Ignore opacity semantics while retaining ordinary channel associations
//! (ISO 32000-2 7.4.9, Table 87; T.800 I.5.3.6).
use crate::jpx;
use pagura_core::{Error, Limits, Result};

pub(super) fn discard_opacity(
    data: &[u8],
    decoded: &mut jpx::Decoded,
    limits: &Limits,
) -> Result<()> {
    let Some(header) = find(data, *b"jp2h")? else {
        return Ok(());
    };
    let Some(definitions) = find(header, *b"cdef")? else {
        return Ok(());
    };
    let count = decoded.image.components.len();
    if decoded.component_geometry.len() != count {
        return Err(Error::syntax("invalid JPX component geometry count"));
    }
    let bytes = decoded
        .image
        .components
        .iter()
        .try_fold(0usize, |sum, c| {
            sum.checked_add(c.samples.len().checked_mul(2)?)
        })
        .and_then(|sum| {
            sum.checked_add(count.checked_mul(
                size_of::<jpx::JpxComponent>()
                    + size_of::<jpx::ComponentGeometry>()
                    + 3 * size_of::<usize>(),
            )?)
        });
    let profile_bytes = match &decoded.image.color {
        jpx::JpxColor::Icc(profile) => profile.len(),
        _ => 0,
    };
    if bytes
        .and_then(|n| n.checked_add(profile_bytes))
        .is_none_or(|n| n > limits.decoded_bytes)
    {
        return Err(Error::limit("JPX ignored opacity metadata budget exceeded"));
    }
    let mut rank: Vec<_> = (count..2 * count).collect();
    let entries = definitions
        .get(2..)
        .ok_or(Error::syntax("short JPX channel definitions"))?;
    let mut associated = false;
    for record in entries.as_chunks::<6>().0 {
        let channel = usize::from(u16::from_be_bytes([record[0], record[1]]));
        let kind = u16::from_be_bytes([record[2], record[3]]);
        let association = u16::from_be_bytes([record[4], record[5]]);
        let value = rank
            .get_mut(channel)
            .ok_or(Error::syntax("invalid JPX channel index"))?;
        if matches!(kind, 1 | 2) {
            if *value < count {
                return Err(Error::syntax("conflicting JPX channel classifications"));
            }
            *value = usize::MAX;
        } else if kind == 0 && association != 0 && association != u16::MAX {
            if *value < count || *value == usize::MAX {
                return Err(Error::syntax("conflicting JPX channel classifications"));
            }
            *value = usize::from(association - 1);
            associated = true;
        }
    }
    let keep = rank.iter().filter(|&&r| r != usize::MAX).count();
    let mut seen = vec![false; keep];
    let (mut color, mut opacity) = (0, keep);
    for target in &mut rank {
        if *target == usize::MAX {
            *target = opacity;
            opacity += 1;
        } else if associated {
            let slot = seen.get_mut(*target).ok_or(Error::unsupported(
                crate::features::JPX_SAMPLE_LAYOUT,
                "incomplete JPX ordinary channel associations",
            ))?;
            if std::mem::replace(slot, true) {
                return Err(Error::syntax("repeated JPX ordinary channel association"));
            }
        } else {
            *target = color;
            color += 1;
        }
    }
    for i in 0..count {
        while rank[i] != i {
            let j = rank[i];
            decoded.image.components.swap(i, j);
            decoded.component_geometry.swap(i, j);
            rank.swap(i, j);
        }
    }
    decoded.image.components.truncate(keep);
    decoded.component_geometry.truncate(keep);
    Ok(())
}

// The codec has already validated the container. Repeat only bounded box
// navigation to recover the channel classifications suppressed by its option.
fn find(mut data: &[u8], kind: [u8; 4]) -> Result<Option<&[u8]>> {
    while !data.is_empty() {
        let header = data.get(..8).ok_or(Error::syntax("short JPX box"))?;
        let short = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let (length, start) = match short {
            0 => (data.len(), 8),
            1 => {
                let bytes = data
                    .get(8..16)
                    .ok_or(Error::syntax("short JPX extended box"))?;
                let bytes: [u8; 8] = bytes
                    .try_into()
                    .map_err(|_| Error::syntax("short JPX box length"))?;
                (
                    usize::try_from(u64::from_be_bytes(bytes))
                        .map_err(|_| Error::limit("JPX box overflow"))?,
                    16,
                )
            }
            n => (
                usize::try_from(n).map_err(|_| Error::limit("JPX box overflow"))?,
                8,
            ),
        };
        let payload = data
            .get(start..length)
            .ok_or(Error::syntax("invalid JPX box length"))?;
        if header[4..8] == kind {
            return Ok(Some(payload));
        }
        data = data
            .get(length..)
            .ok_or(Error::syntax("invalid JPX box length"))?;
    }
    Ok(None)
}
