use super::{BoxData, Budget, reader_requirements};
use pagura_core::{ErrorKind, Limits};

#[test]
fn explicit_lab_metadata_uses_one_budgeted_colour_record() {
    use super::{ColorSelection, FileType, Header, JpxColor, Options, Reader};
    let mut fields = vec![1, 0, 1, 0, 0, 0, 14];
    for value in [100_u32, 0, 170, 128, 200, 96, 0x0044_3530] {
        fields.extend(value.to_be_bytes());
    }
    let bytes = size_of::<super::ColorSpecification>();
    for (decoded_bytes, operations, valid) in
        [(bytes, 1, true), (bytes - 1, 1, false), (bytes, 0, false)]
    {
        let limits = Limits {
            decoded_bytes,
            operations,
            ..Limits::default()
        };
        let mut header = Header::default();
        let result = header.read_color(
            &mut Reader::new(&fields, 80),
            &mut Budget::new(&limits),
            &Options::default(),
            ColorSelection::Pdf,
            FileType::JpxBaseline,
        );
        if valid {
            result.unwrap();
            let JpxColor::Lab(parameters) = &header.colors[0].color else {
                panic!("missing Lab parameters")
            };
            assert_eq!(parameters.ranges(), [100, 170, 200]);
            assert_eq!(parameters.offsets(), [0, 128, 96]);
            assert_eq!(parameters.illuminant(), 0x0044_3530);
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        }
    }
}

#[test]
fn reader_expression_inspects_exactly_its_bounded_body() {
    // ML=1, FUAM=DCM=1, one standard feature (5, mask1), no vendor UUIDs.
    let fields = [1, 1, 1, 0, 1, 0, 5, 1, 0, 0];
    let box_data = BoxData {
        kind: b"rreq",
        body: &fields,
        position: 40,
        to_end: false,
    };
    let limits = Limits {
        operations: 10,
        ..Limits::default()
    };
    reader_requirements(&box_data, &mut Budget::new(&limits)).unwrap();
    let limited = Limits {
        operations: 9,
        ..limits
    };
    assert_eq!(
        reader_requirements(&box_data, &mut Budget::new(&limited))
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut data = u32::try_from(body.len() + 8)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    data.extend(kind);
    data.extend(body);
    data
}

fn dimensions(components: u16) -> Vec<u8> {
    let mut data = 1_u32.to_be_bytes().to_vec();
    data.extend(4_u32.to_be_bytes());
    data.extend(components.to_be_bytes());
    data.extend([7, 7, 0, 0]);
    boxed(b"ihdr", &data)
}

fn color() -> Vec<u8> {
    boxed(b"colr", &[1, 0, 1, 0, 0, 0, 16])
}

/// Box-only framing controls; the codestream is never entropy-decoded here.
fn file(defaults: Option<&[u8]>, extra: &[u8]) -> Vec<u8> {
    let mut data = boxed(b"jP  ", &[13, 10, 135, 10]);
    data.extend(boxed(b"ftyp", b"jpx \0\0\0\x01jpx jpxb"));
    data.extend(boxed(b"rreq", &[1, 1, 1, 0, 1, 0, 5, 1, 0, 0]));
    if let Some(defaults) = defaults {
        data.extend(boxed(b"jp2h", defaults));
    }
    data.extend(extra);
    data.extend(boxed(b"jp2c", &[255, 79, 255, 217]));
    data
}

fn parse(data: &[u8]) -> pagura_core::Result<super::Header> {
    super::parse(
        data,
        &mut Budget::new(&Limits::default()),
        &super::Options::default(),
        super::ColorSelection::Pdf,
    )
    .map(|(_, _, header)| header)
}

#[test]
fn first_headers_inherit_only_missing_owners_and_require_complete_metadata() {
    let dimensions = dimensions(3);
    let color = color();
    let defaults = [dimensions.clone(), color.clone()].concat();
    let empty_headers = [boxed(b"jpch", &[]), boxed(b"jplh", &[])].concat();
    let layer_color = boxed(b"jplh", &boxed(b"cgrp", &color));
    let stream_dimensions = boxed(b"jpch", &dimensions);
    for (defaults, extra) in [
        (defaults.as_slice(), &[][..]),
        (defaults.as_slice(), empty_headers.as_slice()),
        (dimensions.as_slice(), layer_color.as_slice()),
        (color.as_slice(), stream_dimensions.as_slice()),
    ] {
        let header = parse(&file(Some(defaults), extra)).unwrap();
        assert_eq!(header.dimensions, Some((4, 1, 3, 7)));
        assert_eq!(header.colors.len(), 1);
    }
    // Explicitly empty/missing layer colour is not an instruction to guess RGB.
    for data in [
        file(Some(&dimensions), &empty_headers),
        file(Some(&dimensions), &boxed(b"jplh", &boxed(b"cgrp", &[]))),
        file(Some(&color), &empty_headers),
        file(None, &empty_headers),
        file(Some(&defaults), &boxed(b"jpch", &dimensions)),
        file(Some(&defaults), &layer_color),
    ] {
        assert_eq!(parse(&data).err().unwrap().kind, ErrorKind::Syntax);
    }
    // A selected first-header dependency is not made irrelevant by later data.
    let first_extension = boxed(b"jpch", &boxed(b"cref", &[]));
    assert!(matches!(
        parse(&file(Some(&defaults), &first_extension))
            .err()
            .unwrap()
            .kind,
        ErrorKind::Unsupported(_)
    ));
    // Later headers belong to other images. Outer lengths stay checked, but
    // their internal metadata cannot change the selected first image.
    let later_headers = [
        empty_headers,
        boxed(b"jpch", &[255]),
        boxed(b"jplh", &[255]),
    ]
    .concat();
    let header = parse(&file(Some(&defaults), &later_headers)).unwrap();
    assert_eq!(header.dimensions, Some((4, 1, 3, 7)));
}

#[test]
fn ordinary_jp2_header_error_identity_and_order_are_unchanged() {
    let options = super::Options::default();
    let policy = super::HeaderPolicy {
        options: &options,
        selection: super::ColorSelection::Pdf,
        format: super::FileType::Jp2,
    };
    for (body, message) in [
        (color(), "JP2 image header must be first"),
        (
            [dimensions(3), dimensions(3)].concat(),
            "repeated JP2 image header",
        ),
        (dimensions(3), "incomplete JP2 header"),
    ] {
        let mut header = super::Header::default();
        let b = BoxData {
            kind: b"jp2h",
            body: &body,
            position: 80,
            to_end: false,
        };
        let error = header
            .read_boxes(
                &b,
                super::HeaderLocation::Defaults,
                &mut Budget::new(&Limits::default()),
                &policy,
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax);
        assert_eq!(error.message, message);
    }
}

#[test]
fn implicit_opacity_uses_exactly_one_definition_per_final_channel() {
    let options = super::Options::default();
    let policy = super::HeaderPolicy {
        options: &options,
        selection: super::ColorSelection::Pdf,
        format: super::FileType::JpxBaseline,
    };
    // One box inspection plus four initialized cdef tuples: five work units,
    // 4 * 6 = 24 retained bytes. Neither source reading nor channel count grows
    // with the image area, and no sample plane is allocated here.
    for (bytes, operations, valid) in [(24, 5, true), (23, 5, false), (24, 4, false)] {
        let limits = Limits {
            decoded_bytes: bytes,
            operations,
            ..Limits::default()
        };
        let mut budget = Budget::new(&limits);
        let mut header = super::Header {
            dimensions: Some((4, 1, 4, 7)),
            color_present: true,
            baseline_color: true,
            ..super::Header::default()
        };
        let body = boxed(b"opct", &[1]);
        let b = BoxData {
            kind: b"jplh",
            body: &body,
            position: 80,
            to_end: false,
        };
        let result = header
            .read_boxes(&b, super::HeaderLocation::Layer, &mut budget, &policy)
            .and_then(|()| header.finish_first_image(&mut budget));
        if valid {
            result.unwrap();
            assert_eq!(
                header.channels.as_deref(),
                Some(&[(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 2, 0)][..])
            );
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        }
    }
}

#[test]
fn identity_registration_has_a_bounded_extent_and_refuses_other_dependencies() {
    let identity = [0, 1, 0, 1, 0, 0, 1, 1, 0, 0];
    for (operations, valid) in [(10, true), (9, false)] {
        let limits = Limits {
            operations,
            ..Limits::default()
        };
        let result = super::read_registration(
            &mut super::Reader::new(&identity, 80),
            &mut Budget::new(&limits),
        );
        if valid {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        }
    }
    let defaults = [dimensions(3), color()].concat();
    for body in [&identity[..9], &[0; 10][..]] {
        let extra = boxed(b"jplh", &boxed(b"creg", body));
        assert_eq!(
            parse(&file(Some(&defaults), &extra)).err().unwrap().kind,
            ErrorKind::Syntax
        );
    }
    for index in [5, 8, 9] {
        let mut other = identity;
        other[index] = 1;
        let extra = boxed(b"jplh", &boxed(b"creg", &other));
        assert!(matches!(
            parse(&file(Some(&defaults), &extra)).err().unwrap().kind,
            ErrorKind::Unsupported(_)
        ));
    }
}

#[test]
fn first_image_depth_ownership_and_default_opacity_override_are_explicit() {
    let mut variable = dimensions(3);
    variable[18] = 255; // ihdr BPC; its bpcc must be in the same superbox.
    let depths = boxed(b"bpcc", &[7, 11, 15]);
    let complete = [variable.clone(), depths.clone(), color()].concat();
    parse(&file(Some(&complete), &[])).unwrap();
    for defaults in [complete, [variable, color()].concat()] {
        let misplaced = boxed(b"jpch", &depths);
        assert_eq!(
            parse(&file(Some(&defaults), &misplaced))
                .err()
                .unwrap()
                .kind,
            ErrorKind::Syntax
        );
    }
    let mut cdef = 4_u16.to_be_bytes().to_vec();
    for row in [[0_u16, 0, 1], [1, 0, 2], [2, 0, 3], [3, 1, 0]] {
        cdef.extend(row.into_iter().flat_map(u16::to_be_bytes));
    }
    let channels = boxed(b"cdef", &cdef);
    let opacity = boxed(b"opct", &[1]);
    let options = super::Options::default();
    let policy = super::HeaderPolicy {
        options: &options,
        selection: super::ColorSelection::Pdf,
        format: super::FileType::JpxBaseline,
    };
    let mut header = super::Header::default();
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    let defaults = [dimensions(4), color(), channels.clone()].concat();
    let b = BoxData {
        kind: b"jp2h",
        body: &defaults,
        position: 80,
        to_end: false,
    };
    header
        .read_boxes(&b, super::HeaderLocation::Defaults, &mut budget, &policy)
        .unwrap();
    let layer = BoxData {
        kind: b"jplh",
        body: &opacity,
        position: 160,
        to_end: false,
    };
    header
        .read_boxes(&layer, super::HeaderLocation::Layer, &mut budget, &policy)
        .unwrap();
    header.finish_first_image(&mut budget).unwrap();
    assert_eq!(header.channels.as_ref().unwrap()[3], (3, 2, 0));
    for body in [
        [channels.clone(), opacity.clone()].concat(),
        [opacity, channels].concat(),
    ] {
        let mut header = super::Header::default();
        let layer = BoxData {
            kind: b"jplh",
            body: &body,
            position: 160,
            to_end: false,
        };
        assert_eq!(
            header
                .read_boxes(&layer, super::HeaderLocation::Layer, &mut budget, &policy)
                .unwrap_err()
                .kind,
            ErrorKind::Syntax
        );
    }
}
