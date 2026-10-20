//! Lazy root-command bounds shared by the existing compositor policies.
//!
//! This first slice memoizes the hull already computed by `regions`; it does
//! not select or reorder commands, visit geometry eagerly, or change work
//! charges. One instance belongs to one immutable root command list and its
//! canonical page-to-device transform for one render call. Nested command
//! ordinals must never address it. Visibility and padding remain in `regions`.
//!
//! Storage is fixed executor stack scratch, not decoded heap storage: 64 hulls
//! (2,048 bytes), a 64-bit occupancy mask and an admission flag, plus alignment.
//! `size_of::<SceneIndex>()` reports the target's actual size. Disabled instances
//! have the same stack footprint. This allocates no heap storage and cannot
//! consume the semantic surface allowance. It is not a process memory cap.
use crate::display::GlyphRun;
use pagura_raster::{Point, Transform};

pub(super) const CAPACITY: usize = 64;

pub(super) struct SceneIndex {
    hulls: [[Point; 2]; CAPACITY],
    populated: u64,
    enabled: bool,
}

impl SceneIndex {
    /// The caller admits only repeated regional execution worth memoizing.
    /// Hulls are inserted lazily, after the existing semantic work charges.
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            hulls: [[Point::new(0.0, 0.0); 2]; CAPACITY],
            populated: 0,
            enabled,
        }
    }

    /// Enable reuse before the first command visit, once region sizes are known.
    pub(super) fn enable(&mut self, repeated_regions: bool) {
        self.enabled = repeated_regions;
    }

    /// Reuse only root-local, multi-source hulls after semantic admission.
    pub(super) fn hull(
        &mut self,
        run: &GlyphRun,
        device: Transform,
        ordinal: Option<usize>,
    ) -> [Point; 2] {
        let ordinal = ordinal.filter(|&i| run.sources.len() >= 2 && self.admits(i));
        if let Some(hull) = ordinal.and_then(|i| self.get(i)) {
            return hull;
        }
        let hull = run.device_hull(device);
        if let Some(i) = ordinal {
            self.insert(i, hull);
        }
        hull
    }

    /// Cheap admission for the fixed ordinal prefix and repeated execution.
    pub(super) fn admits(&self, ordinal: usize) -> bool {
        self.enabled && ordinal < CAPACITY
    }

    /// Direct lookup by root command ordinal, never by glyph-run ordinal.
    pub(super) fn get(&self, ordinal: usize) -> Option<[Point; 2]> {
        if ordinal >= CAPACITY || self.populated & (1_u64 << ordinal) == 0 {
            return None;
        }
        Some(self.hulls[ordinal])
    }

    /// Unknown, overflowing and empty hulls keep the existing computation.
    /// Refusal is optional and has no diagnostic or resource-budget effect.
    pub(super) fn insert(&mut self, ordinal: usize, hull: [Point; 2]) -> bool {
        if !self.enabled
            || ordinal >= CAPACITY
            || hull.iter().any(|p| !p.is_finite())
            || hull[0].x > hull[1].x
            || hull[0].y > hull[1].y
        {
            return false;
        }
        self.hulls[ordinal] = hull;
        self.populated |= 1_u64 << ordinal;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hull(offset: f64) -> [Point; 2] {
        [Point::new(offset, -2.5), Point::new(offset + 3.0, 4.25)]
    }

    #[test]
    fn root_ordinals_remain_sparse_and_independent() {
        let mut index = SceneIndex::new(true);
        assert_eq!(index.get(0), None);
        assert!(index.insert(3, hull(3.0)));
        assert!(index.insert(CAPACITY - 1, hull(63.0)));
        assert_eq!(index.get(0), None);
        assert_eq!(index.get(2), None);
        assert_eq!(index.get(3), Some(hull(3.0)));
        assert_eq!(index.get(CAPACITY - 1), Some(hull(63.0)));
        // A new render has no entries from the preceding list or transform.
        assert_eq!(SceneIndex::new(true).get(3), None);
    }

    #[test]
    fn disabled_and_out_of_capacity_entries_decline_without_aliasing() {
        let mut disabled = SceneIndex::new(false);
        assert!(!disabled.insert(0, hull(0.0)));
        assert_eq!(disabled.get(0), None);

        let mut index = SceneIndex::new(true);
        assert!(index.insert(0, hull(0.0)));
        for ordinal in [CAPACITY, CAPACITY + 1, usize::MAX] {
            assert!(!index.insert(ordinal, hull(1.0)));
            assert_eq!(index.get(ordinal), None);
        }
        assert_eq!(index.get(0), Some(hull(0.0)));
    }

    #[test]
    fn nonfinite_and_inverted_bounds_are_not_published() {
        let mut index = SceneIndex::new(true);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for coordinate in 0..4 {
                let mut candidate = hull(0.0);
                match coordinate {
                    0 => candidate[0].x = value,
                    1 => candidate[0].y = value,
                    2 => candidate[1].x = value,
                    _ => candidate[1].y = value,
                }
                assert!(!index.insert(5, candidate));
                assert_eq!(index.get(5), None);
            }
        }
        for candidate in [
            [Point::new(2.0, 0.0), Point::new(1.0, 1.0)],
            [Point::new(0.0, 2.0), Point::new(1.0, 1.0)],
        ] {
            assert!(!index.insert(5, candidate));
            assert_eq!(index.get(5), None);
        }
    }

    #[test]
    fn finite_bounds_keep_bits_and_degenerate_extents() {
        let candidate = [
            Point::new(-0.0, f64::from_bits(1)),
            Point::new(0.0, f64::from_bits(1)),
        ];
        let mut index = SceneIndex::new(true);
        assert!(index.insert(0, candidate));
        let actual = index.get(0).unwrap();
        for (actual, expected) in actual.into_iter().zip(candidate) {
            assert_eq!(actual.x.to_bits(), expected.x.to_bits());
            assert_eq!(actual.y.to_bits(), expected.y.to_bits());
        }
    }

    #[test]
    fn storage_has_a_fixed_small_stack_bound() {
        let payload = CAPACITY * size_of::<[Point; 2]>() + size_of::<u64>() + size_of::<bool>();
        let actual = size_of::<SceneIndex>();
        assert!(actual >= payload);
        assert!(actual < payload + align_of::<SceneIndex>());
        assert!(actual <= 2_080);
    }
}
