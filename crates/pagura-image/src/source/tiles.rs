//! Explicit bounded packed surfaces on one logical mask grid (ADR 0010, 0016).

#[cfg(test)]
mod tests;

use super::header::Header;
use super::{Orientation, SampleRect};
use crate::deferred::Packed;
use crate::{DecodedImage, ImageKind, MaskKind, Samples};
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Meter, Result, Value};

/// One decoded surface and its bounds in a collection's oriented sample grid.
/// The surface obeys the individual pixel cap; its bounds never include row
/// padding. Fields are private because collection construction validates them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ImageTile {
    bounds: SampleRect,
    image: DecodedImage,
}

impl ImageTile {
    /// Bounds within the common oriented logical grid, top row first.
    pub fn bounds(&self) -> SampleRect {
        self.bounds
    }

    /// The independently bounded decoded surface, with the mask's entries.
    pub fn image(&self) -> &DecodedImage {
        &self.image
    }
}

/// A complete one-bit gray mask represented by multiple bounded surfaces.
///
/// Dimensions describe the common oriented logical grid, not one oversized
/// decoded surface. The collection has no operation materializing that grid
/// as a whole `DecodedImage`. Every tile and aggregate reservation is validated
/// before construction; fields remain private to preserve those invariants.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DecodedTiles {
    width: u32,
    height: u32,
    rows_per_tile: u32,
    interpolate: bool,
    tiles: Vec<ImageTile>,
}

impl DecodedTiles {
    /// Width of the oriented logical sample grid.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of the oriented logical sample grid.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether the common logical grid requests interpolated reconstruction.
    pub fn interpolate(&self) -> bool {
        self.interpolate
    }

    /// Complete-row surfaces in increasing row order, with no gaps/overlap.
    pub fn tiles(&self) -> &[ImageTile] {
        &self.tiles
    }

    /// One packed logical row. Its last byte may contain padding, which is
    /// never part of the declared `width()` samples.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let tile = self.tiles.get(u32_to_usize(y / self.rows_per_tile))?;
        let Samples::Packed(packed) = &tile.image.samples else {
            return None;
        };
        packed.row(u32_to_usize(y.checked_sub(tile.bounds.y0)?))
    }

    /// The one-bit code at logical (`x`, `y`), excluding all padding.
    pub fn get(&self, x: u32, y: u32) -> Option<u16> {
        if x >= self.width {
            return None;
        }
        let x = u32_to_usize(x);
        self.row(y)?
            .get(x / 8)
            .map(|byte| u16::from((byte >> (7 - x % 8)) & 1))
    }

    /// Actual retained capacities, including spare tile records and decode
    /// arrays. Known one-bit gray tile kinds have inline DeviceGray names.
    pub fn retained_bytes(&self) -> usize {
        let mut bytes = size_of::<Self>()
            .saturating_add(self.tiles.capacity().saturating_mul(size_of::<ImageTile>()));
        for tile in &self.tiles {
            bytes = bytes.saturating_add(crate::deferred::storage_bytes(&tile.image.samples));
            bytes = bytes.saturating_add(tile.image.decode.as_ref().map_or(0, |decode| {
                decode.capacity().saturating_mul(size_of::<f64>())
            }));
        }
        bytes
    }
}

/// Checked complete-collection allocation bounds, without decoding or allocation.
/// These do not authorize decoding; the source revalidates every request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TileLayout {
    height: u32,
    retained: usize,
    peak: usize,
    index_bytes: usize,
    index_work: usize,
}

impl TileLayout {
    /// Original/caller intersected allowance for decoded owners plus a new
    /// column index, cumulative across aliases of the same held source.
    pub const fn index_byte_limit(&self) -> usize {
        self.index_bytes
    }

    /// Original/caller intersected allowance for new index construction and
    /// converted/future indexed queries. Does not cap old-route query work.
    pub const fn index_work_limit(&self) -> usize {
        self.index_work
    }

    /// Positive rows of each full surface; the last can be shorter.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Conservative complete retained collection capacity, excluding Arc counters.
    pub const fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// Peak new collection heap storage, excluding already held source input
    /// and Arc counters. The image layer still validates that source input.
    pub const fn new_peak_bytes(&self) -> usize {
        self.peak
    }
}

/// Complete retained input, including source/header/admission metadata.
#[derive(Debug, Clone, Copy)]
pub(super) struct InputSize {
    pub(super) data: usize,
    pub(super) retained: usize,
}

/// The original document's admission stays binding after it is dropped.
#[derive(Debug, Clone)]
pub(crate) struct Admission {
    original: Limits,
    encoded_bytes: usize,
}

impl Admission {
    pub(super) const fn new(original: &Limits, encoded_bytes: usize) -> Self {
        Self {
            original: *original,
            encoded_bytes,
        }
    }

    pub(super) fn limits_for(&self, requested: &Limits) -> Result<Limits> {
        let original = &self.original;
        let limits = Limits {
            pixels: requested.pixels.min(original.pixels),
            stream_bytes: requested.stream_bytes.min(original.stream_bytes),
            decoded_bytes: requested.decoded_bytes.min(original.decoded_bytes),
            // Only the opaque index allowance reads these intersections;
            // existing tile decode and old-route query gates are unchanged.
            render_bytes: requested.render_bytes.min(original.render_bytes),
            render_work: requested.render_work.min(original.render_work),
            decode_bytes: requested.decode_bytes.min(original.decode_bytes),
            tiled_images: pagura_core::TiledImageLimits {
                samples: requested
                    .tiled_images
                    .samples
                    .min(original.tiled_images.samples),
                work: requested.tiled_images.work.min(original.tiled_images.work),
            },
            ..*requested
        };
        Meter::new(
            limits.stream_bytes,
            "tiled image source stream budget exceeded",
        )
        .charge(self.encoded_bytes)?;
        Ok(limits)
    }
}

/// Predicted complete-collection admission, also checked before cache lookup.
#[derive(Debug)]
pub(super) struct Plan {
    width: u32,
    height: u32,
    stride: usize,
    rows: u32,
    count: usize,
    input_stride: usize,
    retained: usize,
}

impl Plan {
    /// Positive complete rows in a full surface, before the final short one.
    pub(super) const fn layout(&self, limits: &Limits) -> TileLayout {
        TileLayout {
            height: self.rows,
            retained: self.retained,
            // Every output vector starts empty and is reserved once; there is
            // no additional heap scratch during row copy or direct transpose.
            peak: self.retained,
            index_bytes: limits.render_bytes,
            index_work: limits.render_work,
        }
    }

    pub(super) fn new(
        header: &Header,
        input: InputSize,
        orientation: Orientation,
        limits: &Limits,
    ) -> Result<Self> {
        let (width, height) = match orientation {
            Orientation::Rows => (header.width, header.height),
            Orientation::Transposed => (header.height, header.width),
        };
        if width == 0 || height == 0 {
            return Err(Error::syntax("invalid tiled image dimensions"));
        }
        let mut samples = Meter::new(
            u32_to_usize(limits.tiled_images.samples),
            "tiled image sample budget exceeded",
        );
        samples.charge_product(u32_to_usize(width), u32_to_usize(height))?;
        let rows = limits
            .pixels
            .checked_div(u32_to_usize(width))
            .filter(|&rows| rows > 0)
            .ok_or(Error::limit("tiled image row exceeds surface pixel budget"))?
            .min(u32_to_usize(height));
        let rows =
            u32::try_from(rows).map_err(|_| Error::limit("tiled image row count overflow"))?;
        let stride = u32_to_usize(width).div_ceil(8);
        let input_stride = u32_to_usize(header.width).div_ceil(8);
        let expected = input_stride
            .checked_mul(u32_to_usize(header.height))
            .ok_or(Error::limit("tiled image source byte count overflow"))?;
        if expected != input.data {
            return Err(Error::syntax("tiled image requires exact complete rows"));
        }
        let count = u32_to_usize(height.div_ceil(rows));
        let mut plan = Self {
            width,
            height,
            stride,
            rows,
            count,
            input_stride,
            retained: 0,
        };
        plan.reserve_work(header, orientation, limits)?;
        let mut auxiliary = Meter::new(
            limits.decode_bytes,
            "tiled image auxiliary byte budget exceeded",
        );
        auxiliary.charge(input.retained)?;
        // Output/data/metadata use one aggregate meter during construction;
        // this preflight checks the same conservative first allocations.
        let mut output = Meter::new(
            limits.decoded_bytes,
            "tiled image output byte budget exceeded",
        );
        output.charge_items::<DecodedTiles>(1)?;
        output.charge_items::<ImageTile>(count.max(4))?;
        let mut y = 0;
        while y < height {
            let take = rows.min(height - y);
            let bytes = stride
                .checked_mul(u32_to_usize(take))
                .ok_or(Error::limit("tiled image surface byte count overflow"))?;
            output.charge(bytes.max(8))?;
            if header.decode.is_some() {
                output.charge_items::<f64>(4)?;
            }
            y = y
                .checked_add(take)
                .ok_or(Error::limit("tiled image row origin overflow"))?;
        }
        plan.retained = output.used();
        Ok(plan)
    }

    fn reserve_work(
        &self,
        header: &Header,
        orientation: Orientation,
        limits: &Limits,
    ) -> Result<()> {
        let mut work = Meter::new(
            u32_to_usize(limits.tiled_images.work),
            "tiled image construction work budget exceeded",
        );
        match orientation {
            Orientation::Rows => {
                let bytes = self
                    .stride
                    .checked_mul(u32_to_usize(self.height))
                    .ok_or(Error::limit("tiled image output byte count overflow"))?;
                work.charge_product(bytes, 2)?;
            }
            Orientation::Transposed => {
                let samples = u32_to_usize(header.width)
                    .checked_mul(u32_to_usize(header.height))
                    .ok_or(Error::limit("tiled image sample count overflow"))?;
                work.charge_product(samples, 2)?;
                work.charge_product(self.stride, u32_to_usize(self.height))?;
            }
        }
        work.charge(u32_to_usize(header.height))?;
        work.charge(self.count)
    }

    pub(super) fn decode(
        self,
        header: &Header,
        input: &[u8],
        orientation: Orientation,
        limits: &Limits,
    ) -> Result<DecodedTiles> {
        let mut output = Meter::new(
            limits.decoded_bytes,
            "tiled image output byte budget exceeded",
        );
        output.charge_items::<DecodedTiles>(1)?;
        let mut tiles = Vec::new();
        output.reserve_capacity(&mut tiles, self.count)?;
        let mut y = 0;
        while y < self.height {
            let height = self.rows.min(self.height - y);
            let length = self
                .stride
                .checked_mul(u32_to_usize(height))
                .ok_or(Error::limit("tiled image surface byte count overflow"))?;
            let mut bytes = Vec::new();
            output.reserve_capacity(&mut bytes, length)?;
            match orientation {
                Orientation::Rows => {
                    let first = self
                        .stride
                        .checked_mul(u32_to_usize(y))
                        .ok_or(Error::limit("tiled image source index overflow"))?;
                    let end = first
                        .checked_add(length)
                        .ok_or(Error::limit("tiled image source index overflow"))?;
                    bytes.extend_from_slice(
                        input
                            .get(first..end)
                            .ok_or(Error::syntax("tiled image source rows truncated"))?,
                    );
                }
                Orientation::Transposed => bytes.resize(length, 0),
            }
            let decode = if let Some(values) = &header.decode {
                let mut decoded = Vec::new();
                output.reserve_capacity(&mut decoded, values.len())?;
                decoded.extend_from_slice(values);
                Some(decoded)
            } else {
                None
            };
            let end = y
                .checked_add(height)
                .ok_or(Error::limit("tiled image row origin overflow"))?;
            tiles.push(ImageTile {
                bounds: SampleRect {
                    x0: 0,
                    y0: y,
                    x1: self.width,
                    y1: end,
                },
                image: DecodedImage {
                    width: self.width,
                    height,
                    bits_per_component: 1,
                    components: 1,
                    samples: Samples::Packed(Packed {
                        width: u32_to_usize(self.width),
                        height: u32_to_usize(height),
                        stride: self.stride,
                        bytes,
                    }),
                    kind: ImageKind::Color {
                        color_space: Some(Value::name(b"DeviceGray")),
                    },
                    decode,
                    interpolate: header.interpolate,
                    mask: MaskKind::None,
                },
            });
            y = end;
        }
        if orientation == Orientation::Transposed {
            self.transpose(header, input, &mut tiles)?;
        }
        let result = DecodedTiles {
            width: self.width,
            height: self.height,
            rows_per_tile: self.rows,
            interpolate: header.interpolate,
            tiles,
        };
        if result.retained_bytes() > self.retained {
            return Err(Error::limit(
                "tiled image capacity exceeds its validated layout",
            ));
        }
        if result.retained_bytes() > limits.decoded_bytes {
            return Err(Error::limit("tiled image retained capacity exceeded"));
        }
        Ok(result)
    }

    fn transpose(&self, header: &Header, input: &[u8], tiles: &mut [ImageTile]) -> Result<()> {
        for y in 0..header.height {
            let start = u32_to_usize(y)
                .checked_mul(self.input_stride)
                .ok_or(Error::limit("tiled image transpose source index overflow"))?;
            for x in 0..header.width {
                let column = u32_to_usize(x);
                let source = start
                    .checked_add(column / 8)
                    .and_then(|index| input.get(index))
                    .ok_or(Error::syntax("tiled image transpose source truncated"))?;
                let value = (source >> (7 - column % 8)) & 1;
                let tile = tiles
                    .get_mut(u32_to_usize(x / self.rows))
                    .ok_or(Error::limit("tiled image transpose tile index overflow"))?;
                let row = x
                    .checked_sub(tile.bounds.y0)
                    .ok_or(Error::limit("tiled image transpose row index overflow"))?;
                let index = u32_to_usize(row)
                    .checked_mul(self.stride)
                    .and_then(|index| index.checked_add(u32_to_usize(y) / 8))
                    .ok_or(Error::limit("tiled image transpose index overflow"))?;
                if let Samples::Packed(packed) = &mut tile.image.samples {
                    let output = packed
                        .bytes
                        .get_mut(index)
                        .ok_or(Error::limit("tiled image transpose output index overflow"))?;
                    *output |= value << (7 - u32_to_usize(y) % 8);
                }
            }
        }
        Ok(())
    }
}
