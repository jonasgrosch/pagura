//! Approximate signed cells (the `approximate_coverage` fast parameter).

use super::edges::{Bounds, Polygon};
use super::shape::Shape;
use super::{BandCoverage, ShapeOptions};
use crate::{FillRule, Path, Transform};
use pagura_core::{Error, Meter, Result};

#[derive(Debug)]
pub(crate) struct Cells;

#[derive(Debug)]
pub(crate) struct Product;

impl Product {
    pub(crate) fn new(
        _paths: &[(&Path, FillRule)],
        _transform: Transform,
        _rect: Option<Bounds>,
        _options: &ShapeOptions<'_>,
        _bounds: super::DeviceRect,
    ) -> Result<Self> {
        Err(Error::invalid_argument("approximate clips are not implemented"))
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        0
    }
    pub(crate) fn work(&self) -> usize {
        0
    }
}

#[derive(Debug, Default)]
pub(crate) struct Scratch;

impl Scratch {
    pub(crate) fn new(_width: usize, _bytes: &mut Meter) -> Result<Self> {
        Ok(Self)
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        0
    }
    pub(crate) fn rasterize(
        &mut self,
        _cells: &Cells,
        _shape: &Shape,
        _rows: std::ops::Range<u32>,
        _out: &mut BandCoverage,
    ) -> usize {
        0
    }
}

pub(crate) fn fill(
    _path: &Path,
    _transform: Transform,
    _rule: FillRule,
    _options: &ShapeOptions<'_>,
) -> Result<Option<Shape>> {
    Err(Error::invalid_argument("approximate fills are not implemented"))
}

pub(crate) fn stroke(
    _outline: &Path,
    _transform: Transform,
    _pieces: bool,
    _options: &ShapeOptions<'_>,
) -> Result<Option<Shape>> {
    Err(Error::invalid_argument("approximate strokes are not implemented"))
}

pub(crate) fn glyph(_polygon: Polygon, _options: &ShapeOptions<'_>) -> Result<Option<Shape>> {
    Err(Error::invalid_argument("approximate glyphs are not implemented"))
}
