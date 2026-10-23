//! Source instancing and row indices; no repeated glyph/row edge payloads.
use super::{Builder, EDGE, index};
use crate::{PreparedScene, plan::Mark};
use pagura_core::{Error, Feature, Result};
use pagura_raster::{DeviceRect, FillRule, Point};

impl Builder {
    pub(super) fn shared_mark(
        &mut self,
        prepared: &PreparedScene,
        key: Option<u32>,
        mark: &Mark,
    ) -> Result<()> {
        let shape = key
            .and_then(|key| prepared.shapes.shapes.get(key as usize))
            .ok_or(Error::unsupported(
                Feature::new("gpu.raster"),
                "unbounded GPU mark is not implemented",
            ))?;
        let bounds = shape.bounds().intersect(&self.region);
        if bounds.is_empty() {
            return Ok(());
        }
        let geometry = shape.gpu_geometry();
        let origin = [bounds.x0, bounds.y0];
        if let Some((source, placement)) = geometry.shared() {
            // Pointer identity is only a lookup key. Serialized ranges are
            // assigned in first-use order, independent of address/order.
            let key = std::ptr::from_ref(source);
            let range = match self.sources.binary_search_by_key(&key, |&(key, _)| key) {
                Ok(i) => self.sources[i].1,
                Err(i) => {
                    let start = index(self.edge_refs.len())?;
                    source.gpu_edges(|points, curve| {
                        let edge = self.edge(points, curve, [0, 0])?;
                        self.reference(edge)?;
                        Ok(())
                    })?;
                    let range = [start, index(self.edge_refs.len())?];
                    self.bytes.reserve(&mut self.sources, 1)?;
                    self.sources.insert(i, (key, range));
                    range
                }
            };
            self.shared_part(
                shape,
                bounds,
                mark,
                range,
                origin,
                [
                    placement.x - f64::from(origin[0]),
                    placement.y - f64::from(origin[1]),
                ],
                true,
            )?;
        } else {
            self.indexed(shape, bounds, mark, origin)?;
        }
        Ok(())
    }

    fn indexed(
        &mut self,
        shape: &pagura_raster::Shape,
        bounds: DeviceRect,
        mark: &Mark,
        origin: [u32; 2],
    ) -> Result<()> {
        let geometry = shape.gpu_geometry();
        let mut count = 0;
        geometry.edges(|_, _| {
            self.work.charge(1)?;
            count += 1;
            Ok(())
        })?;
        let height = if count > 128 { 4 } else { bounds.height() };
        let rows = bounds.height().div_ceil(height) as usize;
        let mut buckets = Vec::new();
        self.bytes.reserve(&mut buckets, rows)?;
        buckets.resize_with(rows, Vec::<u32>::new);
        let mut temporary = rows * size_of::<Vec<u32>>();
        geometry.edges(|points, curve| {
            self.work.charge(1)?;
            let top = points
                .iter()
                .map(|p| p.y)
                .fold(f64::INFINITY, f64::min)
                .max(f64::from(bounds.y0));
            let bottom = points
                .iter()
                .map(|p| p.y)
                .fold(f64::NEG_INFINITY, f64::max)
                .min(f64::from(bounds.y1));
            if top >= bottom {
                return Ok(());
            }
            let edge = self.edge(points, curve, origin)?;
            let start = ((top - f64::from(bounds.y0)) / f64::from(height)).floor();
            let end = ((bottom - f64::from(bounds.y0)) / f64::from(height)).ceil();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "row indices are finite and clamped to the allocated buckets"
            )]
            let (start, end) = ((start as usize).min(rows), (end as usize).min(rows));
            for bucket in &mut buckets[start..end] {
                self.work.charge(1)?;
                self.bytes.reserve(bucket, 1)?;
                bucket.push(edge);
                temporary += size_of::<u32>();
            }
            Ok(())
        })?;
        for (row, bucket) in buckets.into_iter().enumerate() {
            let start = index(self.edge_refs.len())?;
            for edge in bucket {
                self.reference(edge)?;
            }
            let range = [start, index(self.edge_refs.len())?];
            let y0 = bounds.y0 + index(row)? * height;
            self.shared_part(
                shape,
                DeviceRect {
                    y0,
                    y1: (y0 + height).min(bounds.y1),
                    ..bounds
                },
                mark,
                range,
                origin,
                [0.0; 2],
                false,
            )?;
        }
        self.bytes.refund(temporary);
        Ok(())
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "intentional finite f64-to-f32 experiment"
    )]
    fn edge(&mut self, points: [Point; 3], curve: bool, origin: [u32; 2]) -> Result<u32> {
        self.work.charge(1)?;
        let id = index(self.edges.len() / EDGE)?;
        self.bytes.reserve(&mut self.edges, EDGE)?;
        self.edges.push(u32::from(curve));
        for p in points {
            for (v, o) in [(p.x, origin[0]), (p.y, origin[1])] {
                let v = (v - f64::from(o)) as f32;
                if !v.is_finite() {
                    return Err(Error::limit("shared GPU coordinate exceeds f32"));
                }
                self.edges.push(v.to_bits());
            }
        }
        self.edges.push(0);
        Ok(id)
    }

    fn reference(&mut self, edge: u32) -> Result<()> {
        self.work.charge(1)?;
        self.bytes.reserve(&mut self.edge_refs, 1)?;
        self.edge_refs.push(edge);
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one placement or row slice and its shared geometry"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite local placement narrowed intentionally"
    )]
    fn shared_part(
        &mut self,
        shape: &pagura_raster::Shape,
        bounds: DeviceRect,
        mark: &Mark,
        range: [u32; 2],
        origin: [u32; 2],
        offset: [f64; 2],
        instanced: bool,
    ) -> Result<()> {
        let geometry = shape.gpu_geometry();
        let corrections = index(self.extras.len() / 4)?;
        for y in bounds.y0..bounds.y1 {
            self.work.charge(1)?;
            for &(x, value) in geometry.corrections(y) {
                if x >= bounds.x0 && x < bounds.x1 {
                    self.extra([y, x, x + 1, u32::from(value)])?;
                    self.correction_cells += 1;
                }
            }
        }
        let clips = index(self.extras.len() / 4)?;
        if geometry.has_clip_rows() {
            for y in bounds.y0..bounds.y1 {
                self.work.charge(1)?;
                geometry.clips(y, |x0, x1, value| {
                    let (x0, x1) = (x0.max(bounds.x0), x1.min(bounds.x1));
                    if x0 < x1 {
                        self.extra([y, x0, x1, u32::from(value)])?;
                        self.clip_runs += 1;
                    }
                    Ok(())
                })?;
            }
        }
        let end = index(self.extras.len() / 4)?;
        let color = mark
            .words
            .ok_or(Error::invalid_argument("shared GPU missing solid color"))?;
        let id = index(self.marks.len() / 20)?;
        self.bytes.reserve(&mut self.marks, 20)?;
        self.marks.extend([
            bounds.x0,
            bounds.y0,
            bounds.x1,
            bounds.y1,
            range[0],
            range[1],
            u32::from(geometry.has_clip_rows()),
            u32::from(geometry.rule() == FillRule::EvenOdd),
            origin[0],
            origin[1],
            u32::from(color[0]) | u32::from(color[1]) << 16,
            u32::from(color[2]) | u32::from(color[3]) << 16,
            corrections,
            clips,
            clips,
            end,
            (offset[0] as f32).to_bits(),
            (offset[1] as f32).to_bits(),
            u32::from(instanced),
            0,
        ]);
        self.bin(bounds, id, range[1] - range[0])
    }
}
