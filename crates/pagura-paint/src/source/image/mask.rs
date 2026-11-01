//! One logical opacity grid, independently of its bounded physical surfaces.

use super::column_index::ColumnIndex;
use super::geometry::Geometry;
use super::geometry::Mode;
use super::sampling::{Sampling, Scratch};
use super::texels::{Opacity, OpacityTexels, TiledOpacityTexels};
use super::tiles::OwnerToken;
use super::{Description, MaskDescription, Orientation, PaintContext, Pixels, TiledAdmission};
use pagura_core::{Error, Meter, Result};
use pagura_image::{Decoded, DecodedImage, DecodedTiles, ImageSource};
use std::sync::Arc;

/// The semantic mask description is shared by admission and sampling.
pub(super) fn description(
    mask: &pagura_interpret::ImageMask,
) -> (Option<&Arc<pagura_interpret::Image>>, MaskDescription<'_>) {
    use pagura_interpret::ImageMask;
    match mask {
        ImageMask::Stencil(mask) => (Some(mask), MaskDescription::Stencil),
        ImageMask::Soft { mask, matte } => (
            Some(mask),
            MaskDescription::Soft {
                decode: mask.decode.first().copied().unwrap_or([0.0, 1.0]),
                matte: matte.as_deref(),
            },
        ),
        ImageMask::None | ImageMask::ColorKey(_) | ImageMask::Embedded => {
            (None, MaskDescription::None)
        }
    }
}

/// Admission runs before lookup, including original source limits. Different
/// valid caller pixel caps can require different physical surface heights.
pub(super) fn decode_tiles(
    source: &Arc<ImageSource>,
    description: MaskDescription<'_>,
    orientation: Orientation,
    context: &PaintContext<'_>,
    (tiles, bytes, work): (&mut TiledAdmission<'_>, &mut Meter, &mut Meter),
) -> Result<DecodedMask> {
    let MaskDescription::Soft {
        decode,
        matte: None,
    } = description
    else {
        return Err(Error::syntax("tiled source requires an unmatted soft mask"));
    };
    if decode.iter().any(|value| !value.is_finite()) {
        return Err(Error::syntax("invalid tiled mask Decode range"));
    }
    tiles.decode(source, orientation, context, (bytes, work))
}

#[derive(Debug)]
pub(super) enum DecodedMask {
    Whole(Arc<Decoded>),
    Tiles {
        decoded: Arc<DecodedTiles>,
        owner: Option<OwnerToken>,
        index: Option<Arc<ColumnIndex>>,
    },
}

impl DecodedMask {
    /// Ordinary/fused/Matte consumers only receive a whole decoded surface.
    pub(super) fn whole(&self) -> Option<&DecodedImage> {
        match self {
            Self::Whole(decoded) => Some(&decoded.image),
            Self::Tiles { .. } => None,
        }
    }

    /// Whole masks retain their conservative per-sampler charge. Tiled
    /// collections have a separate physically shared owner in HeldTiles.
    pub(super) fn owned_bytes(&self) -> usize {
        match self {
            Self::Whole(decoded) => super::decoded_bytes(&decoded.image),
            Self::Tiles { .. } => 0,
        }
    }

    fn placement(
        &self,
        inverse: pagura_raster::Transform,
        orientation: Orientation,
    ) -> Result<Geometry> {
        match self {
            Self::Whole(decoded) => super::placement(&decoded.image, inverse, orientation),
            Self::Tiles { decoded, .. } => match orientation {
                Orientation::Rows => Geometry::new(
                    (decoded.width(), decoded.height()),
                    decoded.interpolate(),
                    inverse,
                ),
                Orientation::Transposed => Ok(Geometry::new(
                    (decoded.height(), decoded.width()),
                    decoded.interpolate(),
                    inverse,
                )?
                .transposed()),
            },
        }
    }
}

#[derive(Debug)]
pub(super) struct MaskSampling {
    pub(super) image: DecodedMask,
    pub(super) opacity: Opacity,
    /// Explicit stencils and alpha-is-shape masks scale the image's shape.
    pub(super) scales_shape: bool,
    /// A tiled mask always reconstructs on its complete logical grid.
    pub(super) independent: Option<(Geometry, Sampling)>,
}

impl MaskSampling {
    pub(super) fn new(
        image: DecodedMask,
        description: &Description<'_>,
        parent: &DecodedImage,
        (inverse, orientation): (pagura_raster::Transform, Orientation),
        (geometry, columns): (&Geometry, (u32, u32)),
        bytes: &mut Meter,
    ) -> Result<Self> {
        let (decode, scales_shape) = match description.mask {
            MaskDescription::Soft { decode, .. } => (decode, description.alpha_is_shape),
            _ => ([0.0, 1.0], true),
        };
        let (opacity, fused) = match &image {
            DecodedMask::Whole(decoded) => {
                super::validate(&decoded.image)?;
                let fused = (decoded.image.width, decoded.image.height)
                    == (parent.width, parent.height)
                    && (decoded.image.interpolate == parent.interpolate
                        || geometry.mode() == Mode::Area);
                (Opacity::new(&decoded.image, decode, bytes)?, fused)
            }
            DecodedMask::Tiles { .. } => {
                if !matches!(description.mask, MaskDescription::Soft { matte: None, .. }) {
                    return Err(Error::syntax("tiled source requires an unmatted soft mask"));
                }
                (Opacity::tiled(decode, bytes)?, false)
            }
        };
        let independent = if fused {
            None
        } else {
            let geometry = image.placement(inverse, orientation)?;
            let plan = Sampling::new(&geometry, columns, bytes)?;
            Some((geometry, plan))
        };
        Ok(Self {
            image,
            opacity,
            scales_shape,
            independent,
        })
    }

    pub(super) fn cost(&self, parent: &Geometry) -> usize {
        self.independent
            .as_ref()
            .map_or(parent.cost(), |(geometry, _)| geometry.cost())
    }

    /// Only exact binary area samplers can own replaceable reservations.
    pub(super) fn index_candidate(
        &self,
        pixels: usize,
    ) -> Option<(OwnerToken, &Arc<DecodedTiles>, usize)> {
        let DecodedMask::Tiles {
            decoded,
            owner: Some(owner),
            ..
        } = &self.image
        else {
            return None;
        };
        let (geometry, Sampling::Separable(_)) = self.independent.as_ref()? else {
            return None;
        };
        if geometry.mode() != Mode::Area || self.opacity.polarity().is_none() {
            return None;
        }
        Some((
            *owner,
            decoded,
            pixels
                .saturating_mul(geometry.column_cost())
                .saturating_mul(64),
        ))
    }

    pub(super) fn clear_index_owner(&mut self) {
        if let DecodedMask::Tiles { owner, .. } = &mut self.image {
            *owner = None;
        }
    }

    pub(super) fn attach_index(&mut self, value: Arc<ColumnIndex>) {
        if let DecodedMask::Tiles { index, .. } = &mut self.image {
            *index = Some(value);
        }
    }

    pub(super) fn fill(&self, pixels: Pixels<'_>, scratch: &mut Scratch, out: &mut [f64]) {
        let Some((geometry, plan)) = &self.independent else {
            return;
        };
        match &self.image {
            DecodedMask::Whole(decoded) => plan.fill(
                geometry,
                pixels,
                scratch,
                &OpacityTexels {
                    mask: &decoded.image,
                    opacity: &self.opacity,
                },
                out,
            ),
            DecodedMask::Tiles { decoded, index, .. } => plan.fill(
                geometry,
                pixels,
                scratch,
                &TiledOpacityTexels {
                    mask: decoded,
                    opacity: &self.opacity,
                    index: index.as_deref(),
                    polarity: self.opacity.polarity(),
                },
                out,
            ),
        }
    }
}
