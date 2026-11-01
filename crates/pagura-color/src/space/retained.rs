//! Retained resolved colour-space ownership, including shared child roots.

use super::{ColorSpace, Colorant};
use crate::icc::Profile;
use std::sync::Arc;

const ARC: usize = 2 * size_of::<usize>();

impl ColorSpace {
    /// Bytes retained by this value and its owned data, including `Self`.
    /// Shared child spaces, profiles and functions are counted per reference,
    /// with Arc counters. This is a conservative ownership estimate; it does
    /// not depend on reference counts or allocator telemetry.
    pub fn retained_bytes(&self) -> usize {
        self.retained(true)
    }

    /// [`Self::retained_bytes`] without the ICC profiles along the chain and
    /// their Arc counters, for an owner that charges each profile once.
    /// Summed without them, not taken from the saturating total.
    pub fn retained_bytes_without_profiles(&self) -> usize {
        self.retained(false)
    }

    fn retained(&self, profiles: bool) -> usize {
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
                            .filter(|_| profiles)
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

    /// The ICC profiles along this space and its chain of child spaces,
    /// each of which [`Self::retained_bytes`] counts with its Arc counter.
    pub fn profiles(&self) -> impl Iterator<Item = &Arc<Profile>> {
        std::iter::successors(Some(self), |space| match space {
            Self::Icc(space) => Some(space.alternate.as_ref()),
            Self::Indexed(space) => Some(space.base.as_ref()),
            Self::Separation(space) => Some(space.alternate.as_ref()),
            Self::DeviceN(space) => Some(space.alternate.as_ref()),
            Self::Pattern(base) => base.as_deref(),
            Self::DeviceGray
            | Self::DeviceRgb
            | Self::DeviceCmyk
            | Self::Sycc
            | Self::Calibrated(_) => None,
        })
        .filter_map(|space| match space {
            Self::Icc(space) => space.profile.as_ref(),
            _ => None,
        })
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
    fn profiles_are_found_along_the_chain() {
        let bytes = pagura_testkit::fixture("color", "sampled-rgb.icc");
        let profile = Arc::new(
            Profile::parse(&bytes, 3, &pagura_core::Limits::default())
                .unwrap()
                .unwrap(),
        );
        let icc = Arc::new(ColorSpace::Icc(Arc::new(crate::IccSpace {
            profile: Some(Arc::clone(&profile)),
            alternate: Arc::new(ColorSpace::DeviceRgb),
            components: 3,
            range: [[0.0, 1.0]; 4],
            object: None,
        })));
        let indexed = ColorSpace::Indexed(Arc::new(Indexed {
            base: Arc::clone(&icc),
            high: 0,
            lookup: vec![0; 3].into(),
        }));
        let pattern = ColorSpace::Pattern(Some(Arc::new(indexed.clone())));
        for space in [icc.as_ref(), &indexed, &pattern] {
            let found: Vec<_> = space.profiles().collect();
            assert_eq!(found.len(), 1);
            assert!(Arc::ptr_eq(found[0], &profile));
        }
        assert_eq!(ColorSpace::DeviceRgb.profiles().count(), 0);
        assert_eq!(ColorSpace::Pattern(None).profiles().count(), 0);
    }

    /// The CMYK lut16 fixture with an A2B0 of 33^4 CLUT nodes, which hold
    /// about 29 MB.
    fn large_profile() -> Profile {
        let fixture = pagura_testkit::fixture("color", "lut16-cmyk-xyz.icc");
        let word = |at: usize| {
            usize::try_from(u32::from_be_bytes(fixture[at..at + 4].try_into().unwrap())).unwrap()
        };
        let mut lut = b"mft2\0\0\0\0".to_vec();
        lut.extend([4, 3, 33, 0]);
        for i in 0..9 {
            lut.extend(if i % 4 == 0 { 0x1_0000_i32 } else { 0 }.to_be_bytes());
        }
        lut.extend([0, 2, 0, 2]);
        // Two-sample identity curves around a CLUT of zeros.
        lut.extend([0, 0, 255, 255].repeat(4));
        lut.resize(lut.len() + 33_usize.pow(4) * 3 * 2, 0);
        lut.extend([0, 0, 255, 255].repeat(3));
        let tags: Vec<_> = (0..word(128))
            .map(|i| {
                let at = 132 + 12 * i;
                let data = match &fixture[at..at + 4] {
                    b"A2B0" => lut.clone(),
                    _ => fixture[word(at + 4)..word(at + 4) + word(at + 8)].to_vec(),
                };
                (&fixture[at..at + 4], data)
            })
            .collect();
        let mut entries = fixture[..132].to_vec();
        let mut payload = Vec::new();
        let start = 132 + 12 * tags.len();
        for (signature, data) in &tags {
            entries.extend_from_slice(signature);
            entries.extend(u32::try_from(start + payload.len()).unwrap().to_be_bytes());
            entries.extend(u32::try_from(data.len()).unwrap().to_be_bytes());
            payload.extend(data);
            payload.resize(payload.len().next_multiple_of(4), 0);
        }
        entries.extend(payload);
        let size = u32::try_from(entries.len()).unwrap();
        entries[..4].copy_from_slice(&size.to_be_bytes());
        Profile::parse(&entries, 4, &pagura_core::Limits::default())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn bytes_without_profiles_are_summed_without_them() {
        // ICC spaces that each reference one large profile, more than 4 GiB
        // of them per reference: a total that saturates a 32-bit usize, from
        // which taking the profiles back out would undercount the records.
        let profile = Arc::new(large_profile());
        let each = ARC + profile.retained_bytes();
        assert!(each > 16 << 20, "{each} bytes per reference");
        let layers = usize::try_from(u32::MAX).unwrap() / each + 2;
        let chain = |profile: Option<&Arc<Profile>>| {
            (0..layers).fold(Arc::new(ColorSpace::DeviceCmyk), |alternate, _| {
                Arc::new(ColorSpace::Icc(Arc::new(crate::IccSpace {
                    profile: profile.cloned(),
                    alternate,
                    components: 4,
                    range: [[0.0, 1.0]; 4],
                    object: None,
                })))
            })
        };
        let (shared, bare) = (chain(Some(&profile)), chain(None));
        assert_eq!(shared.profiles().count(), layers);
        assert_eq!(
            shared.retained_bytes_without_profiles(),
            bare.retained_bytes()
        );
        if cfg!(target_pointer_width = "32") {
            assert_eq!(shared.retained_bytes(), usize::MAX);
        } else {
            assert_eq!(
                shared.retained_bytes(),
                bare.retained_bytes() + layers * each
            );
        }
    }

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
