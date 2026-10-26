use super::{Dict, Kind, Operator};
use crate::budget::{FontBudget, Work};
use crate::read::cursor;
use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};

fn parse(data: &[u8], kind: Kind<'_>, limits: &Limits) -> pagura_core::Result<Dict> {
    let mut diagnostics = Diagnostics::new(Recovery::Strict);
    Dict::parse(
        cursor(data).with_base(100),
        kind,
        &mut Work::new(FontBudget::new(limits), &mut diagnostics),
    )
}

#[test]
fn blends_preserve_preceding_operands_and_feed_dict_operators() {
    // The first coefficient is zero at default, the second one is one.
    let data = [144, 149, 159, 169, 140, 23, 6]; // 5,10,20,30,1 blend BlueValues
    let regions = [vec![0.0, 1.0]];
    let dict = parse(&data, Kind::Cff2Private(&regions), &Limits::default()).unwrap();
    assert_eq!(
        dict.values(Operator::BLUE_VALUES),
        Some([5.0, 40.0].as_slice())
    );
    let small = Limits {
        font_entries: 1,
        ..Limits::default()
    };
    let one = [vec![1.0]];
    let error = parse(&[139, 19, 139, 22], Kind::Cff2Private(&one), &small).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    for malformed in [&[140, 23][..], &[140, 22][..], &[139, 23][..]] {
        let e = parse(malformed, Kind::Cff2Private(&one), &Limits::default()).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Syntax);
        assert!(matches!(e.position, Position::Buffer(_)));
    }
}

#[test]
fn reals_numbers_and_stack_ceilings() {
    // BlueScale (12 9) of -2.25: nibbles e 2 a 2 5 f.
    let dict = parse(
        &[30, 0xe2, 0xa2, 0x5f, 12, 9],
        Kind::Cff,
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(dict.number(Operator::BLUE_SCALE, 0.0).unwrap(), -2.25);
    let dict = parse(&[30, 0x1c, 0x3f, 12, 9], Kind::Cff, &Limits::default()).unwrap();
    assert_eq!(dict.number(Operator::BLUE_SCALE, 0.0).unwrap(), 1e-3);
    for malformed in [
        &[30, 0xf1][..],        // the high nibble ends but the low one is not padding
        &[30, 0xd0, 12, 9][..], // reserved nibble
        &[31, 6][..],           // reserved number byte
        &[139, 6, 139, 6][..],  // duplicate operator
        &[139][..],             // operands without an operator
    ] {
        let error = parse(malformed, Kind::Cff, &Limits::default()).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{malformed:?}");
    }
    let mut long = vec![30];
    long.extend([0x11; 40]);
    long.push(0xff);
    assert_eq!(
        parse(&long, Kind::Cff, &Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
    let mut stack = vec![139; 49];
    stack.push(6);
    let error = parse(&stack, Kind::Cff, &Limits::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Syntax);
    stack.truncate(48);
    stack.push(6);
    parse(&stack, Kind::Cff, &Limits::default()).unwrap();
}

#[test]
fn font_dict_discards_unknown_operands_before_the_private_entry() {
    // Repeated legacy FontMatrix operators intentionally have the wrong
    // arity. In a current FontDICT they are unknown, so neither is retained
    // or validated as a matrix, and neither contaminates the Private pair.
    let data = [140, 12, 7, 140, 12, 7, 139, 139, 18];
    let limits = Limits {
        font_work: 7,
        font_entries: 1,
        ..Limits::default()
    };
    let dict = parse(&data, Kind::Cff2Font, &limits).unwrap();
    assert_eq!(dict.entries.len(), 1);
    assert_eq!(dict.values(Operator::PRIVATE), Some([0.0, 0.0].as_slice()));
    assert!(!dict.has(Operator::FONT_MATRIX));
    let short_work = Limits {
        font_work: 6,
        ..limits
    };
    assert_eq!(
        parse(&data, Kind::Cff2Font, &short_work).unwrap_err().kind,
        ErrorKind::Limit
    );
    let no_entries = Limits {
        font_entries: 0,
        ..limits
    };
    assert_eq!(
        parse(&data, Kind::Cff2Font, &no_entries).unwrap_err().kind,
        ErrorKind::Limit
    );
    parse(&data[..6], Kind::Cff2Font, &no_entries).unwrap();
}

#[test]
fn ignored_font_operators_still_require_well_formed_bounded_numbers() {
    let limits = Limits {
        font_entries: 0,
        ..Limits::default()
    };
    for malformed in [
        &[12][..],
        &[139][..],
        &[31, 12, 7][..],
        &[30, 0xd0, 12, 7][..],
    ] {
        let error = parse(malformed, Kind::Cff2Font, &limits).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Syntax, "{malformed:?}");
        assert!(matches!(error.position, Position::Buffer(_)));
    }
    let mut full = vec![139; 513];
    full.extend([12, 255]); // An unrecognized escaped operator, not FontMatrix.
    parse(&full, Kind::Cff2Font, &limits).unwrap();
    full.insert(0, 139);
    assert_eq!(
        parse(&full, Kind::Cff2Font, &limits).unwrap_err().kind,
        ErrorKind::Syntax
    );
    let duplicate_private = [139, 139, 18, 139, 139, 18];
    assert_eq!(
        parse(&duplicate_private, Kind::Cff2Font, &Limits::default())
            .unwrap_err()
            .kind,
        ErrorKind::Syntax
    );
}

#[test]
fn font_dict_context_does_not_change_top_or_cff1_matrices() {
    use pagura_raster::Transform;
    let matrix = [141, 139, 139, 141, 139, 139, 12, 7]; // [2 0 0 2 0 0]
    for kind in [Kind::Cff, Kind::Cff2Top] {
        let dict = parse(&matrix, kind, &Limits::default()).unwrap();
        assert_eq!(
            dict.matrix(Transform::IDENTITY).unwrap(),
            Transform::scale(2.0, 2.0)
        );
    }
    let mut diagnostics = Diagnostics::new(Recovery::Repair);
    let dict = Dict::parse(
        cursor(&matrix).with_base(100),
        Kind::Cff2Font,
        &mut Work::new(FontBudget::new(&Limits::default()), &mut diagnostics),
    )
    .unwrap();
    assert_eq!(
        dict.matrix(Transform::IDENTITY).unwrap(),
        Transform::IDENTITY
    );
    assert!(diagnostics.take().is_empty()); // Normative ignoring is not a repair.
}
