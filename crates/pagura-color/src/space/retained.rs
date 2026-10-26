//! Retained resolved colour-space ownership, including shared child roots.

use super::{ColorSpace, Colorant};

const ARC: usize = 2 * size_of::<usize>();

impl ColorSpace {
    /// Bytes retained by this value and its owned data, including `Self`.
    /// Shared child spaces, profiles and functions are counted per reference,
    /// with Arc counters. This is a conservative ownership estimate; it does
    /// not depend on reference counts or allocator telemetry.
    pub fn retained_bytes(&self) -> usize {
        // Every family has at most one child colour space. Walk it without
        // recursion: these public records may form a long chain.
        let mut current = self;
        let mut total = 0_usize;
        loop {
            total = total.saturating_add(size_of::<Self>());
            let (payload, next) = match current {
                Self::DeviceGray
                | Self::DeviceRgb
                | Self::DeviceCmyk
                | Self::Sycc
                | Self::Pattern(None) => (0, None),
                Self::Calibrated(space) => (ARC.saturating_add(size_of_val(space.as_ref())), None),
                Self::Icc(space) => (
                    [
                        ARC,
                        size_of_val(space.as_ref()),
                        ARC,
                        space
                            .profile
                            .as_ref()
                            .map_or(0, |profile| ARC.saturating_add(profile.retained_bytes())),
                    ]
                    .into_iter()
                    .fold(0, usize::saturating_add),
                    Some(space.alternate.as_ref()),
                ),
                Self::Indexed(space) => (
                    [
                        ARC,
                        size_of_val(space.as_ref()),
                        ARC,
                        ARC,
                        space.lookup.len(),
                    ]
                    .into_iter()
                    .fold(0, usize::saturating_add),
                    Some(space.base.as_ref()),
                ),
                Self::Separation(space) => (
                    [
                        ARC,
                        size_of_val(space.as_ref()),
                        name(&space.colorant),
                        ARC,
                        ARC,
                        space.tint.retained_bytes(),
                    ]
                    .into_iter()
                    .fold(0, usize::saturating_add),
                    Some(space.alternate.as_ref()),
                ),
                Self::DeviceN(space) => (
                    [
                        ARC,
                        size_of_val(space.as_ref()),
                        space
                            .colorants
                            .capacity()
                            .saturating_mul(size_of::<Colorant>()),
                        space
                            .colorants
                            .iter()
                            .map(name)
                            .fold(0, usize::saturating_add),
                        ARC,
                        ARC,
                        space.tint.retained_bytes(),
                    ]
                    .into_iter()
                    .fold(0, usize::saturating_add),
                    Some(space.alternate.as_ref()),
                ),
                Self::Pattern(Some(base)) => (ARC, Some(base.as_ref())),
            };
            total = total.saturating_add(payload);
            let Some(next) = next else {
                return total;
            };
            current = next;
        }
    }
}

fn name(colorant: &Colorant) -> usize {
    match colorant {
        Colorant::Named(name) => name.heap_bytes(),
        Colorant::All | Colorant::None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceN, ExponentialFunction, Function, Indexed};
    use pagura_core::Name;
    use std::sync::Arc;

    #[test]
    fn indexed_and_devicen_owners_include_tables_spare_capacity_and_long_names() {
        let base = Arc::new(ColorSpace::DeviceRgb);
        let indexed = ColorSpace::Indexed(Arc::new(Indexed {
            base,
            high: 1,
            lookup: vec![0; 6].into(),
        }));
        assert_eq!(
            indexed.retained_bytes(),
            size_of::<ColorSpace>()
                + ARC
                + size_of::<Indexed>()
                + ARC
                + size_of::<ColorSpace>()
                + ARC
                + 6
        );
        let tint = Arc::new(
            Function::exponential(ExponentialFunction {
                domain: [0.0, 1.0],
                range: None,
                c0: vec![0.0; 3],
                c1: vec![1.0; 3],
                exponent: 1.0,
            })
            .unwrap(),
        );
        let mut colorants = Vec::with_capacity(7);
        colorants.push(Colorant::Named(Name::new(&[b'a'; 64])));
        let capacity = colorants.capacity();
        let space = ColorSpace::DeviceN(Arc::new(DeviceN {
            colorants,
            alternate: Arc::new(indexed),
            tint: Arc::clone(&tint),
        }));
        let expected = size_of::<ColorSpace>()
            + ARC
            + size_of::<DeviceN>()
            + capacity * size_of::<Colorant>()
            + 64
            + ARC
            + size_of::<ColorSpace>()
            + ARC
            + size_of::<Indexed>()
            + ARC
            + size_of::<ColorSpace>()
            + ARC
            + 6
            + ARC
            + tint.retained_bytes();
        assert_eq!(space.retained_bytes(), expected);
    }

    #[test]
    fn public_space_chains_are_walked_without_recursion() {
        let mut space = Arc::new(ColorSpace::DeviceGray);
        // Keep each predecessor alive so dropping the test graph is bounded,
        // too; the ownership query itself needs no stack or visited storage.
        let mut keep = vec![Arc::clone(&space)];
        for _ in 0..4096 {
            space = Arc::new(ColorSpace::Pattern(Some(space)));
            keep.push(Arc::clone(&space));
        }
        assert_eq!(
            space.retained_bytes(),
            4097 * size_of::<ColorSpace>() + 4096 * ARC
        );
        drop(space);
        while keep.pop().is_some() {}
    }
}
