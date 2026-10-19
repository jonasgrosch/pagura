//! Ignore opacity semantics while retaining ordinary channel associations
//! (ISO 32000-2 7.4.9, Table 87; T.800 I.5.3.6).
use pagura_core::{Error, Limits, Result};
use pagura_imagecodec::jpx::{self, Association, ChannelType};

/// What a channel definition makes of a channel.
#[derive(Clone, Copy)]
enum Role {
    /// Kept, with no color association.
    Unassociated,
    /// Kept as color `n`, counted from zero.
    Color(usize),
    /// Opacity of any kind, which is dropped.
    Opacity,
}

/// Drops the opacity channels of `decoded`, whose channel definitions the
/// codec did not interpret, and orders the other channels by their color
/// associations when the definitions give them.
pub(super) fn discard_opacity(decoded: &mut jpx::Decoded, limits: &Limits) -> Result<()> {
    if decoded.channel_definitions.is_empty() {
        return Ok(());
    }
    let count = decoded.image.components.len();
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
                    + size_of::<Role>()
                    + size_of::<(usize, usize)>()
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
    let mut roles = vec![Role::Unassociated; count];
    let mut associated = false;
    for definition in &decoded.channel_definitions {
        let role = roles
            .get_mut(usize::from(definition.channel))
            .ok_or(Error::syntax("invalid JPX channel index"))?;
        match (definition.kind, definition.association) {
            (ChannelType::Opacity | ChannelType::PremultipliedOpacity, _) => {
                if matches!(role, Role::Color(_)) {
                    return Err(Error::syntax("conflicting JPX channel classifications"));
                }
                *role = Role::Opacity;
            }
            (ChannelType::Color, Association::Color(color)) => {
                if !matches!(role, Role::Unassociated) {
                    return Err(Error::syntax("conflicting JPX channel classifications"));
                }
                *role = Role::Color(usize::from(color.get()) - 1);
                associated = true;
            }
            _ => {}
        }
    }
    let order: Vec<usize> = if associated {
        // Every kept channel needs its own color, and the colors number the
        // kept channels from zero: sorted, they are 0, 1, 2, …
        let mut colors = Vec::with_capacity(count);
        for (channel, role) in roles.iter().enumerate() {
            match *role {
                Role::Color(color) => colors.push((color, channel)),
                Role::Opacity => {}
                Role::Unassociated => return Err(incomplete()),
            }
        }
        if colors.iter().any(|&(color, _)| color >= colors.len()) {
            return Err(incomplete());
        }
        colors.sort_unstable();
        if colors.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Error::syntax("repeated JPX ordinary channel association"));
        }
        colors.into_iter().map(|(_, channel)| channel).collect()
    } else {
        let kept = roles.iter().enumerate();
        kept.filter(|(_, role)| !matches!(role, Role::Opacity))
            .map(|(channel, _)| channel)
            .collect()
    };
    decoded.select_channels(&order)
}

fn incomplete() -> Error {
    Error::unsupported(
        crate::features::JPX_SAMPLE_LAYOUT,
        "incomplete JPX ordinary channel associations",
    )
}
