//! Channel associations and opacity selection (T.800 I.5.3.6; PDF 7.4.9).

use super::io::{Budget, syntax};
use super::{JpxImage, Opacity, OpacityKind, unsupported};
use pagura_core::{Error, Result};

pub(super) fn apply(
    image: &mut JpxImage,
    definitions: &[(u16, u16, u16)],
    origins: &[(usize, Option<u8>)],
    geometry: &mut [super::ComponentGeometry],
    budget: &mut Budget<'_>,
) -> Result<Option<Opacity>> {
    let count = image.components.len();
    let mut opacity = None;
    let mut seen = budget.filled(count, false)?;
    let mut order = budget.filled(count, None)?;
    for &(channel, kind, association) in definitions {
        let index = usize::from(channel);
        let entry = seen
            .get_mut(index)
            .ok_or_else(|| syntax("JP2 channel index out of range", 0))?;
        if *entry {
            return Err(unsupported(
                "multiple descriptions of one JP2 channel are not implemented",
                0,
            ));
        }
        *entry = true;
        match kind {
            0 if association != 0 && association != 65535 => {
                let target = usize::from(association - 1);
                let slot = order
                    .get_mut(target)
                    .ok_or_else(|| syntax("JP2 channel association out of range", 0))?;
                if slot.replace(index).is_some() {
                    return Err(syntax("repeated JP2 colour association", 0));
                }
            }
            0 | 65535 => {
                return Err(unsupported(
                    "unassociated JP2 channels are not implemented",
                    0,
                ));
            }
            1 | 2 if association == 0 => {
                if image.components[index].signed {
                    return Err(syntax("signed JP2 opacity component", 0));
                }
                opacity = Some(Opacity {
                    kind: if kind == 1 {
                        OpacityKind::Straight
                    } else {
                        OpacityKind::Premultiplied
                    },
                    channel: index,
                    source_channel: index,
                    source_component: origins[index].0,
                    palette_column: origins[index].1,
                });
                if image.alpha.replace(index).is_some() {
                    return Err(unsupported(
                        "multiple JP2 opacity channels are not implemented",
                        0,
                    ));
                }
            }
            1 | 2 => return Err(unsupported("per-colour JP2 opacity is not implemented", 0)),
            _ => return Err(syntax("invalid JP2 channel type", 0)),
        }
    }
    // A complete definition is needed to reorder without guessing unspecified associations.
    if seen.contains(&false) {
        return Err(unsupported(
            "partial JP2 channel definitions are not implemented",
            0,
        ));
    }
    let colors = count - usize::from(image.alpha.is_some());
    if order[..colors].contains(&None) || order[colors..].iter().any(Option::is_some) {
        return Err(syntax("incomplete JP2 colour associations", 0));
    }
    if let Some(alpha) = image.alpha {
        order[colors] = Some(alpha);
        image.alpha = Some(colors);
    }
    let mut rank = budget.filled(count, 0)?;
    for (target, source) in order.into_iter().enumerate() {
        let source = source.ok_or_else(|| syntax("missing JP2 channel association", 0))?;
        rank[source] = target;
    }
    permute(image, geometry, &mut rank, budget)?;
    if let Some(opacity) = &mut opacity {
        opacity.channel = colors;
    }
    Ok(opacity)
}

/// PDF SMaskInData 0 ignores opacity semantics, while ordinary channel
/// associations still determine colour order (7.4.9, Table 87).
pub(super) fn discard_opacity(
    image: &mut JpxImage,
    definitions: &[(u16, u16, u16)],
    geometry: &mut Vec<super::ComponentGeometry>,
    budget: &mut Budget<'_>,
) -> Result<()> {
    let count = image.components.len();
    let mut rank = budget.filled(count, count)?;
    for (i, target) in rank.iter_mut().enumerate() {
        *target += i;
    }
    let mut associated = false;
    budget.work(definitions.len())?;
    for &(channel, kind, association) in definitions {
        let value = rank
            .get_mut(usize::from(channel))
            .ok_or_else(|| syntax("invalid JPX channel index", 0))?;
        match kind {
            1 | 2 => {
                if *value < count {
                    return Err(syntax("conflicting JPX channel classifications", 0));
                }
                *value = usize::MAX;
            }
            0 if association != 0 && association != u16::MAX => {
                if *value < count || *value == usize::MAX {
                    return Err(syntax("conflicting JPX channel classifications", 0));
                }
                *value = usize::from(association - 1);
                associated = true;
            }
            0 | 65535 => {}
            _ => return Err(syntax("invalid JP2 channel type", 0)),
        }
    }
    let keep = rank.iter().filter(|&&r| r != usize::MAX).count();
    let mut seen = budget.filled(keep, false)?;
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
                return Err(syntax("repeated JPX ordinary channel association", 0));
            }
        } else {
            *target = color;
            color += 1;
        }
    }
    permute(image, geometry, &mut rank, budget)?;
    image.components.truncate(keep);
    geometry.truncate(keep);
    Ok(())
}

/// Swaps preserve all sample allocations, including large component planes.
fn permute(
    image: &mut JpxImage,
    geometry: &mut [super::ComponentGeometry],
    rank: &mut [usize],
    budget: &mut Budget<'_>,
) -> Result<()> {
    budget.work(rank.len())?;
    for i in 0..rank.len() {
        while rank[i] != i {
            let j = rank[i];
            image.components.swap(i, j);
            geometry.swap(i, j);
            rank.swap(i, j);
        }
    }
    Ok(())
}
