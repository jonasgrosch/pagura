//! Portable pre-code literal-bit words/ranges and Fraction footprint controls.

use super::*;
use pagura_testkit::{Json, fixture};

struct Case {
    name: String,
    width: u32,
    height: u32,
    stride: usize,
    raw: Vec<u8>,
}

impl Case {
    fn load(name: String) -> Self {
        let params = fixture("packed_mask_tiles", &format!("index-{name}.params"));
        let json = Json::parse(std::str::from_utf8(&params).unwrap()).unwrap();
        let number = |key| u32::try_from(json.get(key).unwrap().as_u64().unwrap()).unwrap();
        Self {
            width: number("width"),
            height: number("height"),
            stride: u32_to_usize(number("stride")),
            raw: fixture("packed_mask_tiles", &format!("index-{name}.in")),
            name,
        }
    }

    fn bytes(&self, suffix: &str) -> Vec<u8> {
        fixture(
            "packed_mask_tiles",
            &format!("index-{}.{suffix}", self.name),
        )
    }

    fn row(&self, y: u32) -> Option<&[u8]> {
        let start = u32_to_usize(y) * self.stride;
        self.raw.get(start..start + self.stride)
    }

    fn index(&self) -> ColumnIndex {
        let mut meter = Meter::new(1 << 24, "literal-bit component bytes");
        let index = ColumnIndex::build(
            Layout::new(self.width, self.height).unwrap(),
            &mut meter,
            |y| self.row(y),
        )
        .unwrap();
        assert_eq!(meter.used(), index.bytes());
        index
    }
}

fn u32be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().unwrap())
}

fn ranges(case: &Case, index: &ColumnIndex) -> usize {
    let words = case.bytes("words.u64be");
    assert_eq!(words.as_chunks::<8>().1, []);
    let words: Vec<u64> = words
        .as_chunks::<8>()
        .0
        .iter()
        .map(|b| u64::from_be_bytes(*b))
        .collect();
    let prefix = case.bytes("prefix.u32be");
    assert_eq!(prefix.as_chunks::<4>().1, []);
    let prefix: Vec<u32> = prefix
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_be_bytes(*b))
        .collect();
    assert_eq!(index.words, words, "{} words", case.name);
    assert_eq!(index.prefixes, prefix, "{} prefixes", case.name);
    let ranges = case.bytes("ranges");
    assert_eq!(ranges.len() % 16, 0);
    for q in ranges.as_chunks::<16>().0 {
        assert_eq!(
            index.ones(
                u32_to_usize(u32be(&q[..4])),
                u32be(&q[4..8]),
                u32be(&q[8..12])
            ),
            u32be(&q[12..]),
            "{} range",
            case.name
        );
    }
    ranges.len() / 16
}

fn old_steps(case: &Case, footprint: &Footprint, polarity: Polarity) -> Vec<f64> {
    let mut old = vec![0.0; u32_to_usize(case.width)];
    let mut between = old.clone();
    for y in footprint.first..footprint.end {
        let values: Vec<f64> = (0..u32_to_usize(case.width))
            .map(|x| {
                let bit = (case.row(y).unwrap()[x / 8] >> (7 - x % 8)) & 1;
                f64::from(match polarity {
                    Polarity::Forward => bit,
                    Polarity::Reverse => 1 - bit,
                })
            })
            .collect();
        super::super::separable::old_step_for_index_test(
            (
                u32_to_usize(y),
                u32_to_usize(footprint.first),
                u32_to_usize(footprint.end - 1),
            ),
            (footprint.first_weight, footprint.last_weight),
            (&mut old, &mut between),
            &values,
        );
    }
    old
}

fn roi(case: &Case, index: &ColumnIndex, q: &[f64]) -> f64 {
    let across = Footprint::new(0, q[3] - q[1], q[1], case.height);
    let columns = Footprint::new(0, q[2] - q[0], q[0], case.width);
    if across.first == across.end || columns.first == columns.end {
        return 0.0;
    }
    let polarity = if q[5] == 0.0 {
        Polarity::Forward
    } else {
        Polarity::Reverse
    };
    let mut actual = vec![0.0; u32_to_usize(case.width)];
    index.reduce(
        &across,
        (0, u32_to_usize(case.width)),
        polarity,
        &mut actual,
    );
    let old = old_steps(case, &across, polarity);
    assert_eq!(
        actual.iter().map(|n| n.to_bits()).collect::<Vec<_>>(),
        old.iter().map(|n| n.to_bits()).collect::<Vec<_>>(),
        "{} old column steps",
        case.name
    );
    // This component oracle normalizes each axis before the next. It does
    // not introduce a production sequence or tiny-footprint render claim.
    let lo = q[0].clamp(0.0, f64::from(case.width));
    let hi = q[2].clamp(0.0, f64::from(case.width));
    let mut sum = 0.0;
    for x in columns.first..columns.end {
        let weight = (hi.min(f64::from(x) + 1.0) - lo.max(f64::from(x))).max(0.0);
        sum += (actual[u32_to_usize(x)] * across.normalization) * (weight * columns.normalization);
    }
    sum
}

#[test]
fn all_literal_words_ranges_and_fraction_rois() {
    let mut counts = (0, 0, 0);
    for (width, height) in [
        (1, 7),
        (63, 7),
        (64, 7),
        (65, 7),
        (127, 7),
        (128, 7),
        (129, 7),
        (65, 1),
        (2, 1),
        (1, 1),
    ] {
        for pattern in ["wave", "last-bit", "ones"] {
            for orientation in ["rows", "transposed", "mirrored"] {
                let case = Case::load(format!("w{width}-h{height}-{pattern}-{orientation}"));
                let index = case.index();
                counts.0 += 1;
                counts.1 += ranges(&case, &index);
                let rois = case.bytes("rois");
                assert_eq!(rois.len() % 48, 0);
                for q in rois.as_chunks::<48>().0 {
                    let q: Vec<f64> = q
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|b| f64::from_be_bytes(*b))
                        .collect();
                    let result = roi(&case, &index, &q);
                    assert!(
                        (result - q[4]).abs() <= 1e-12,
                        "{} Fraction ROI: {result} != {}",
                        case.name,
                        q[4]
                    );
                    counts.2 += 1;
                }
            }
        }
    }
    assert_eq!(counts, (90, 41_313, 1_320));
}
