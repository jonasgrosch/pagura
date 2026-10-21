//! Glyph masks reused per origin bin (the `glyph_origin_bins` fast parameter).

use super::shape::Shape;
use super::{BandCoverage, GlyphEdges, ShapeOptions};
use crate::{Path, Point};
use pagura_core::{Error, Limits, Meter, Result};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct Prepared;

impl Prepared {
    pub(crate) fn new(_outline: &Path, _linear: [f64; 4], _scale: f64, _limits: &Limits) -> Result<Self> {
        Err(Error::invalid_argument("binned glyphs are not implemented"))
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        0
    }
}

#[derive(Debug)]
pub(crate) struct Placement;

#[derive(Debug, Default)]
pub(crate) struct Cache;

impl Cache {
    pub(crate) fn new(_bytes: &mut Meter) -> Result<Self> {
        Ok(Self)
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        0
    }
    pub(crate) fn rasterize(
        &mut self,
        _placement: &Placement,
        _shape: &Shape,
        _rows: std::ops::Range<u32>,
        _out: &mut BandCoverage,
    ) -> usize {
        0
    }
}

pub(crate) fn place(
    _edges: &Arc<GlyphEdges>,
    _prepared: &Prepared,
    _origin: Point,
    _options: &ShapeOptions<'_>,
) -> Result<Option<Shape>> {
    Err(Error::invalid_argument("binned glyphs are not implemented"))
}
