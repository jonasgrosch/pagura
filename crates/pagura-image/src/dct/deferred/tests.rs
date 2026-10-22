use super::{Deferred, decode_deferred};
use pagura_core::{Diagnostics, Limits, Recovery};

/// Independently encoded baseline coefficients: Y=(148,8,0,...), Cb DC=-400,
/// Cr DC=400. Table 13 produces the (78,178) green half tie.
const GREEN_TIE: &[u8] = &[
    255, 216, 255, 219, 0, 67, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 255, 192, 0, 17, 8, 0, 8, 0, 8, 3, 1, 17, 0, 2, 17, 0, 3, 17, 0,
    255, 196, 0, 224, 0, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7,
    8, 9, 10, 11, 16, 0, 0, 0, 0, 0, 0, 0, 176, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
    10, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 49, 50, 51,
    52, 53, 54, 55, 56, 57, 58, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 81, 82, 83, 84, 85, 86, 87,
    88, 89, 90, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 113, 114, 115, 116, 117, 118, 119,
    120, 121, 122, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 145, 146, 147, 148, 149, 150,
    151, 152, 153, 154, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 177, 178, 179, 180, 181,
    182, 183, 184, 185, 186, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 209, 210, 211, 212,
    213, 214, 215, 216, 217, 218, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 241, 242, 243,
    244, 245, 246, 247, 248, 249, 250, 0, 16, 32, 48, 64, 80, 96, 112, 128, 144, 160, 176, 192,
    208, 224, 240, 255, 218, 0, 12, 3, 1, 0, 2, 0, 3, 0, 0, 63, 0, 137, 64, 56, 160, 147, 125, 4,
    228, 40, 63, 255, 217,
];

fn deferred(bytes: &[u8], transform: Option<i64>) -> Option<Deferred> {
    decode_deferred(
        bytes,
        transform,
        &Limits::default(),
        super::Native {
            buffers: None,
            cells: 8,
            transposed: false,
        },
        &mut Diagnostics::new(Recovery::Strict),
    )
}

#[test]
fn green_half_ties_take_exact_luma() {
    let reference = super::super::decode(GREEN_TIE, Some(1), &Limits::default()).unwrap();
    let Some(Deferred::Planar(planar)) = deferred(GREEN_TIE, Some(1)) else {
        panic!("expected native planes");
    };
    for (index, &value) in reference.samples.iter().enumerate() {
        assert_eq!(planar.get(index), Some(u16::from(value)), "sample {index}");
    }
}

#[test]
fn rgb_and_ycck_frames_keep_the_ordinary_decoder() {
    for transform in [0u8, 2] {
        let mut marked = GREEN_TIE[..2].to_vec();
        marked.extend_from_slice(&[
            255, 238, 0, 14, 65, 100, 111, 98, 101, 0, 100, 0, 0, 0, 0, transform,
        ]);
        marked.extend_from_slice(&GREEN_TIE[2..]);
        assert!(deferred(&marked, None).is_none());
    }
    assert!(deferred(GREEN_TIE, Some(0)).is_none());
}

#[test]
fn truncated_and_corrupt_frames_are_declined() {
    for end in (2..GREEN_TIE.len()).step_by(7) {
        let result = deferred(&GREEN_TIE[..end], Some(1));
        let ordinary = super::super::decode(&GREEN_TIE[..end], Some(1), &Limits::default());
        if let Some(Deferred::Planar(planar)) = result {
            let reference = ordinary.expect("a deferred decode implies an ordinary one");
            for (index, &value) in reference.samples.iter().enumerate() {
                assert_eq!(planar.get(index), Some(u16::from(value)));
            }
        }
    }
}
