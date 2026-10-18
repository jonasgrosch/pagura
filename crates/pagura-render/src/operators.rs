//! Annex A content operators, decoded once into typed operations.
//!
//! [`Operator::parse`] recognizes an operator keyword; [`Operator::decode`]
//! checks its operands (count and types, Annex A) and returns an
//! [`Operation`] whose handlers never index into the operand list.

use crate::color::MAX_COMPONENTS;
use pagura_core::{Error, Name, Result, Value};
use pagura_raster::{FillRule, Point, Transform};

/// The most operands one operator takes: `scn` with a full DeviceN
/// colour plus a pattern name.
pub(crate) const MAX_OPERANDS: usize = MAX_COMPONENTS + 1;

macro_rules! operators {
    ($($variant:ident = $name:literal,)*) => {
        /// An Annex A content operator.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum Operator {
            $(#[doc = concat!("`", $name, "`")] $variant,)*
        }
        impl Operator {
            /// The operator a content-stream keyword names, if any.
            pub(crate) fn parse(word: &[u8]) -> Option<Self> {
                Some(match word {
                    $(w if w == $name.as_bytes() => Self::$variant,)*
                    _ => return None,
                })
            }
            /// The operator's PDF spelling.
            #[cfg(test)]
            pub(crate) const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
            /// Every operator, in Annex A order.
            #[cfg(test)]
            pub(crate) const ALL: &[Self] = &[$(Self::$variant),*];
        }
    };
}

operators! {
    CloseFillStroke = "b",
    FillStroke = "B",
    CloseFillStrokeEvenOdd = "b*",
    FillStrokeEvenOdd = "B*",
    BeginMarkedProperties = "BDC",
    BeginImage = "BI",
    BeginMarked = "BMC",
    BeginText = "BT",
    BeginCompatibility = "BX",
    CurveTo = "c",
    Concat = "cm",
    StrokeSpace = "CS",
    FillSpace = "cs",
    Dash = "d",
    GlyphWidth = "d0",
    GlyphBox = "d1",
    PaintXObject = "Do",
    MarkPointProperties = "DP",
    EndImage = "EI",
    EndMarked = "EMC",
    EndText = "ET",
    EndCompatibility = "EX",
    Fill = "f",
    FillObsolete = "F",
    FillEvenOdd = "f*",
    StrokeGray = "G",
    FillGray = "g",
    GraphicsState = "gs",
    ClosePath = "h",
    Flatness = "i",
    ImageData = "ID",
    LineJoin = "j",
    LineCap = "J",
    StrokeCmyk = "K",
    FillCmyk = "k",
    LineTo = "l",
    MoveTo = "m",
    MiterLimit = "M",
    MarkPoint = "MP",
    EndPath = "n",
    Save = "q",
    Restore = "Q",
    Rectangle = "re",
    StrokeRgb = "RG",
    FillRgb = "rg",
    Intent = "ri",
    CloseStroke = "s",
    Stroke = "S",
    StrokeColor = "SC",
    StrokeColorN = "SCN",
    FillColor = "sc",
    FillColorN = "scn",
    Shade = "sh",
    NextLine = "T*",
    CharacterSpacing = "Tc",
    MoveText = "Td",
    MoveTextSetLeading = "TD",
    Font = "Tf",
    ShowText = "Tj",
    ShowTextArray = "TJ",
    Leading = "TL",
    TextMatrix = "Tm",
    RenderMode = "Tr",
    Rise = "Ts",
    WordSpacing = "Tw",
    HorizontalScaling = "Tz",
    CurveToInitial = "v",
    LineWidth = "w",
    Clip = "W",
    ClipEvenOdd = "W*",
    CurveToFinal = "y",
    NextLineShowText = "'",
    NextLineShowSpaced = "\"",
}

/// A decoded operation with typed operands.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Operation<'v> {
    Save,
    Restore,
    Concat(Transform),
    Parameter(Parameter<'v>),
    GraphicsState(&'v Name),
    Construct(Construct),
    Paint(Painting),
    Clip(FillRule),
    Color(Target, ColorOperation<'v>),
    Text(TextOperation<'v>),
    Glyph(GlyphMetrics),
    XObject(&'v Name),
    Shade(&'v Name),
    BeginMarked {
        tag: &'v Name,
        properties: Option<&'v Value>,
    },
    EndMarked,
    MarkPoint,
    BeginCompatibility,
    EndCompatibility,
    /// `ID` or `EI` outside an inline image.
    ImageDelimiter,
}

/// A general graphics-state parameter (Table 56), also set by ExtGState.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Parameter<'v> {
    LineWidth(f64),
    LineCap(i64),
    LineJoin(i64),
    MiterLimit(f64),
    Dash { array: &'v [Value], phase: f64 },
    Intent(&'v Name),
    Flatness(f64),
}

/// Path construction in user space (Table 58).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Construct {
    MoveTo(Point),
    LineTo(Point),
    /// `c`: two control points and the end point.
    CurveTo([Point; 3]),
    /// `v`: the current point is the first control point.
    CurveToInitial([Point; 2]),
    /// `y`: the end point is the second control point.
    CurveToFinal([Point; 2]),
    Close,
    Rectangle {
        origin: Point,
        width: f64,
        height: f64,
    },
}

/// A path-painting operator (Table 59): `n` neither fills nor strokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Painting {
    pub(crate) close: bool,
    pub(crate) fill: Option<FillRule>,
    pub(crate) stroke: bool,
}
impl Painting {
    pub(crate) const FILL: Self = Self {
        close: false,
        fill: Some(FillRule::NonZero),
        stroke: false,
    };
    pub(crate) const STROKE: Self = Self {
        close: false,
        fill: None,
        stroke: true,
    };
    pub(crate) const FILL_STROKE: Self = Self {
        close: false,
        fill: Some(FillRule::NonZero),
        stroke: true,
    };
}

/// Which colour an operator sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Fill,
    Stroke,
}

/// A colour operator (Table 73).
#[derive(Debug, Clone, Copy)]
pub(crate) enum ColorOperation<'v> {
    /// `CS`/`cs`: a family or resource name.
    Space(&'v Name),
    /// `G`/`g`, `RG`/`rg`, `K`/`k`: a device space and its components.
    Device(Device, &'v [Value]),
    /// `SC`/`sc`: numeric components.
    Components(&'v [Value]),
    /// `SCN`/`scn`: numeric components, then a pattern name for a Pattern space.
    ComponentsOrPattern(&'v [Value]),
}

/// A device colour space set directly by its operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Device {
    Gray,
    Rgb,
    Cmyk,
}
impl Device {
    pub(crate) const fn name(self) -> &'static [u8] {
        match self {
            Self::Gray => b"DeviceGray",
            Self::Rgb => b"DeviceRGB",
            Self::Cmyk => b"DeviceCMYK",
        }
    }
}

/// A text-state, positioning or showing operator (Tables 103–109).
#[derive(Debug, Clone, Copy)]
pub(crate) enum TextOperation<'v> {
    Begin,
    End,
    CharacterSpacing(f64),
    WordSpacing(f64),
    HorizontalScaling(f64),
    Leading(f64),
    Font {
        name: &'v Name,
        size: f64,
    },
    RenderMode(i64),
    Rise(f64),
    Move {
        x: f64,
        y: f64,
        set_leading: bool,
    },
    Matrix(Transform),
    NextLine,
    Show(&'v [u8]),
    ShowArray(&'v [Value]),
    NextLineShow(&'v [u8]),
    NextLineShowSpaced {
        word: f64,
        character: f64,
        text: &'v [u8],
    },
}

/// Type 3 glyph metrics (Table 111).
#[derive(Debug, Clone, Copy)]
pub(crate) enum GlyphMetrics {
    /// `d0`: the horizontal and vertical advance.
    Width([f64; 2]),
    /// `d1`: the advance and the glyph bounding box.
    Box { width: [f64; 2], bbox: [f64; 4] },
}

impl Operator {
    /// Text-showing and text-state operators (9.3, 9.4), across which
    /// consecutive glyphs stay one display-list run.
    pub(crate) const fn keeps_text_run(self) -> bool {
        matches!(
            self,
            Self::ShowText
                | Self::ShowTextArray
                | Self::NextLineShowText
                | Self::NextLineShowSpaced
                | Self::CharacterSpacing
                | Self::WordSpacing
                | Self::HorizontalScaling
                | Self::Leading
                | Self::Rise
                | Self::Font
                | Self::TextMatrix
                | Self::MoveText
                | Self::MoveTextSetLeading
                | Self::NextLine
        )
    }

    /// Colour-related operators that an uncoloured Type 3 glyph (9.6.4,
    /// Table 111) and an uncoloured tiling cell (8.6.8) ignore: colour and
    /// colour-space setters, `ri` and `sh`.
    pub(crate) const fn uses_color(self) -> bool {
        matches!(
            self,
            Self::StrokeGray
                | Self::FillGray
                | Self::StrokeRgb
                | Self::FillRgb
                | Self::StrokeCmyk
                | Self::FillCmyk
                | Self::StrokeSpace
                | Self::FillSpace
                | Self::StrokeColor
                | Self::StrokeColorN
                | Self::FillColor
                | Self::FillColorN
                | Self::Intent
                | Self::Shade
        )
    }

    /// Checks the operands (Annex A) and decodes the operation.
    ///
    /// # Errors
    /// `Syntax` for a wrong operand count or type.
    pub(crate) fn decode(self, v: &[Value]) -> Result<Operation<'_>> {
        use Operation as O;
        Ok(match self {
            Self::Save => none(v, O::Save)?,
            Self::Restore => none(v, O::Restore)?,
            Self::Concat => O::Concat(matrix(v)?),
            Self::LineWidth => O::Parameter(Parameter::LineWidth(one_number(v)?)),
            Self::LineCap => O::Parameter(Parameter::LineCap(one_integer(v)?)),
            Self::LineJoin => O::Parameter(Parameter::LineJoin(one_integer(v)?)),
            Self::MiterLimit => O::Parameter(Parameter::MiterLimit(one_number(v)?)),
            Self::Flatness => O::Parameter(Parameter::Flatness(one_number(v)?)),
            Self::Intent => O::Parameter(Parameter::Intent(one_name(v)?)),
            Self::Dash => match v {
                [Value::Array(array), phase] => O::Parameter(Parameter::Dash {
                    array,
                    phase: number(phase)?,
                }),
                [_, _] => return Err(type_error()),
                _ => return Err(count_error()),
            },
            Self::GraphicsState => O::GraphicsState(one_name(v)?),
            Self::MoveTo => O::Construct(Construct::MoveTo(point(v)?)),
            Self::LineTo => O::Construct(Construct::LineTo(point(v)?)),
            Self::CurveTo => O::Construct(Construct::CurveTo(points::<3>(v)?)),
            Self::CurveToInitial => O::Construct(Construct::CurveToInitial(points::<2>(v)?)),
            Self::CurveToFinal => O::Construct(Construct::CurveToFinal(points::<2>(v)?)),
            Self::ClosePath => none(v, O::Construct(Construct::Close))?,
            Self::Rectangle => {
                let [x, y, width, height] = numbers::<4>(v)?;
                O::Construct(Construct::Rectangle {
                    origin: Point::new(x, y),
                    width,
                    height,
                })
            }
            Self::Stroke => paint(v, false, None, true)?,
            Self::CloseStroke => paint(v, true, None, true)?,
            Self::Fill | Self::FillObsolete => paint(v, false, Some(FillRule::NonZero), false)?,
            Self::FillEvenOdd => paint(v, false, Some(FillRule::EvenOdd), false)?,
            Self::FillStroke => paint(v, false, Some(FillRule::NonZero), true)?,
            Self::FillStrokeEvenOdd => paint(v, false, Some(FillRule::EvenOdd), true)?,
            Self::CloseFillStroke => paint(v, true, Some(FillRule::NonZero), true)?,
            Self::CloseFillStrokeEvenOdd => paint(v, true, Some(FillRule::EvenOdd), true)?,
            Self::EndPath => paint(v, false, None, false)?,
            Self::Clip => none(v, O::Clip(FillRule::NonZero))?,
            Self::ClipEvenOdd => none(v, O::Clip(FillRule::EvenOdd))?,
            Self::StrokeSpace => O::Color(Target::Stroke, ColorOperation::Space(one_name(v)?)),
            Self::FillSpace => O::Color(Target::Fill, ColorOperation::Space(one_name(v)?)),
            Self::StrokeGray => device(v, Target::Stroke, Device::Gray, 1)?,
            Self::FillGray => device(v, Target::Fill, Device::Gray, 1)?,
            Self::StrokeRgb => device(v, Target::Stroke, Device::Rgb, 3)?,
            Self::FillRgb => device(v, Target::Fill, Device::Rgb, 3)?,
            Self::StrokeCmyk => device(v, Target::Stroke, Device::Cmyk, 4)?,
            Self::FillCmyk => device(v, Target::Fill, Device::Cmyk, 4)?,
            Self::StrokeColor => components(v, Target::Stroke, false)?,
            Self::FillColor => components(v, Target::Fill, false)?,
            Self::StrokeColorN => components(v, Target::Stroke, true)?,
            Self::FillColorN => components(v, Target::Fill, true)?,
            Self::BeginText => none(v, O::Text(TextOperation::Begin))?,
            Self::EndText => none(v, O::Text(TextOperation::End))?,
            Self::CharacterSpacing => O::Text(TextOperation::CharacterSpacing(one_number(v)?)),
            Self::WordSpacing => O::Text(TextOperation::WordSpacing(one_number(v)?)),
            Self::HorizontalScaling => O::Text(TextOperation::HorizontalScaling(one_number(v)?)),
            Self::Leading => O::Text(TextOperation::Leading(one_number(v)?)),
            Self::Rise => O::Text(TextOperation::Rise(one_number(v)?)),
            Self::RenderMode => O::Text(TextOperation::RenderMode(one_integer(v)?)),
            Self::Font => match v {
                [Value::Name(name), size] => O::Text(TextOperation::Font {
                    name,
                    size: number(size)?,
                }),
                [_, _] => return Err(type_error()),
                _ => return Err(count_error()),
            },
            Self::TextMatrix => O::Text(TextOperation::Matrix(matrix(v)?)),
            Self::MoveText | Self::MoveTextSetLeading => {
                let [x, y] = numbers::<2>(v)?;
                O::Text(TextOperation::Move {
                    x,
                    y,
                    set_leading: self == Self::MoveTextSetLeading,
                })
            }
            Self::NextLine => none(v, O::Text(TextOperation::NextLine))?,
            Self::ShowText => O::Text(TextOperation::Show(one_string(v)?)),
            Self::NextLineShowText => O::Text(TextOperation::NextLineShow(one_string(v)?)),
            Self::ShowTextArray => match v {
                [Value::Array(array)] => {
                    if array
                        .iter()
                        .any(|v| v.as_number().is_none() && v.as_string().is_none())
                    {
                        return Err(Error::syntax("TJ array requires numbers or strings"));
                    }
                    O::Text(TextOperation::ShowArray(array))
                }
                [_] => return Err(type_error()),
                _ => return Err(count_error()),
            },
            Self::NextLineShowSpaced => match v {
                [word, character, Value::String(text)] => {
                    O::Text(TextOperation::NextLineShowSpaced {
                        word: number(word)?,
                        character: number(character)?,
                        text,
                    })
                }
                [_, _, _] => return Err(type_error()),
                _ => return Err(count_error()),
            },
            Self::GlyphWidth => O::Glyph(GlyphMetrics::Width(numbers::<2>(v)?)),
            Self::GlyphBox => {
                let [wx, wy, x0, y0, x1, y1] = numbers::<6>(v)?;
                O::Glyph(GlyphMetrics::Box {
                    width: [wx, wy],
                    bbox: [x0, y0, x1, y1],
                })
            }
            Self::PaintXObject => O::XObject(one_name(v)?),
            Self::Shade => O::Shade(one_name(v)?),
            Self::BeginMarked => O::BeginMarked {
                tag: one_name(v)?,
                properties: None,
            },
            Self::BeginMarkedProperties => {
                let (tag, properties) = tag_properties(v)?;
                O::BeginMarked {
                    tag,
                    properties: Some(properties),
                }
            }
            Self::EndMarked => none(v, O::EndMarked)?,
            Self::MarkPoint => {
                one_name(v)?;
                O::MarkPoint
            }
            Self::MarkPointProperties => {
                tag_properties(v)?;
                O::MarkPoint
            }
            Self::BeginCompatibility => none(v, O::BeginCompatibility)?,
            Self::EndCompatibility => none(v, O::EndCompatibility)?,
            Self::BeginImage | Self::ImageData | Self::EndImage => none(v, O::ImageDelimiter)?,
        })
    }
}

fn count_error() -> Error {
    Error::syntax("wrong operand count")
}
fn type_error() -> Error {
    Error::syntax("wrong operand type")
}
fn none<'v>(v: &[Value], operation: Operation<'v>) -> Result<Operation<'v>> {
    if v.is_empty() {
        Ok(operation)
    } else {
        Err(count_error())
    }
}
fn number(value: &Value) -> Result<f64> {
    value.as_number().ok_or_else(type_error)
}
fn numbers<const N: usize>(v: &[Value]) -> Result<[f64; N]> {
    let values: &[Value; N] = v.try_into().map_err(|_| count_error())?;
    let mut out = [0.0; N];
    for (out, value) in out.iter_mut().zip(values) {
        *out = number(value)?;
    }
    Ok(out)
}
fn one_number(v: &[Value]) -> Result<f64> {
    numbers::<1>(v).map(|[n]| n)
}
fn one_integer(v: &[Value]) -> Result<i64> {
    match v {
        [value] => value.as_integer().ok_or_else(type_error),
        _ => Err(count_error()),
    }
}
fn one_name(v: &[Value]) -> Result<&Name> {
    match v {
        [Value::Name(name)] => Ok(name),
        [_] => Err(type_error()),
        _ => Err(count_error()),
    }
}
fn one_string(v: &[Value]) -> Result<&[u8]> {
    match v {
        [Value::String(text)] => Ok(text),
        [_] => Err(type_error()),
        _ => Err(count_error()),
    }
}
fn point(v: &[Value]) -> Result<Point> {
    numbers::<2>(v).map(|[x, y]| Point::new(x, y))
}
fn points<const N: usize>(v: &[Value]) -> Result<[Point; N]> {
    if v.len() != 2 * N {
        return Err(count_error());
    }
    let mut out = [Point::default(); N];
    for (out, pair) in out.iter_mut().zip(v.chunks_exact(2)) {
        *out = point(pair)?;
    }
    Ok(out)
}
fn matrix(v: &[Value]) -> Result<Transform> {
    numbers::<6>(v).map(|[a, b, c, d, e, f]| Transform::new(a, b, c, d, e, f))
}
fn paint(v: &[Value], close: bool, fill: Option<FillRule>, stroke: bool) -> Result<Operation<'_>> {
    none(
        v,
        Operation::Paint(Painting {
            close,
            fill,
            stroke,
        }),
    )
}
fn device(v: &[Value], target: Target, space: Device, count: usize) -> Result<Operation<'_>> {
    if v.len() != count {
        return Err(count_error());
    }
    if v.iter().any(|v| v.as_number().is_none()) {
        return Err(type_error());
    }
    Ok(Operation::Color(target, ColorOperation::Device(space, v)))
}
/// `SC`/`sc` take numbers only; `SCN`/`scn` may end with a pattern name.
/// The selected colour space checks the count.
fn components(v: &[Value], target: Target, pattern: bool) -> Result<Operation<'_>> {
    let numeric = match v.split_last() {
        Some((Value::Name(_), rest)) if pattern => rest,
        _ => v,
    };
    if numeric.iter().any(|v| v.as_number().is_none()) {
        return Err(Error::syntax("wrong colour operand type"));
    }
    Ok(Operation::Color(
        target,
        if pattern {
            ColorOperation::ComponentsOrPattern(v)
        } else {
            ColorOperation::Components(v)
        },
    ))
}
fn tag_properties(v: &[Value]) -> Result<(&Name, &Value)> {
    match v {
        [
            Value::Name(tag),
            properties @ (Value::Name(_) | Value::Dictionary(_)),
        ] => Ok((tag, properties)),
        [_, _] => Err(type_error()),
        _ => Err(count_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_annex_a_operator_parses_back_from_its_name() {
        assert_eq!(Operator::ALL.len(), 73);
        for &operator in Operator::ALL {
            assert_eq!(Operator::parse(operator.name().as_bytes()), Some(operator));
        }
        assert_eq!(Operator::parse(b"future"), None);
        assert_eq!(Operator::parse(b"TJ "), None);
    }

    #[test]
    fn operands_are_checked_before_any_handler_runs() {
        let number = Value::Integer(0);
        for (operator, operands) in [
            (Operator::ShowText, vec![Value::Integer(1)]),
            (Operator::Font, vec![Value::name(b"F")]),
            (Operator::Concat, vec![]),
            (Operator::Save, vec![Value::Integer(1)]),
            (Operator::Dash, vec![number.clone(), number.clone()]),
            (
                Operator::ShowTextArray,
                vec![Value::Array(vec![Value::Null])],
            ),
            (Operator::FillColor, vec![Value::name(b"P")]),
            (
                Operator::FillColorN,
                vec![Value::name(b"P"), number.clone()],
            ),
            (
                Operator::BeginMarkedProperties,
                vec![Value::name(b"T"), number],
            ),
        ] {
            assert_eq!(
                operator.decode(&operands).unwrap_err().kind,
                pagura_core::ErrorKind::Syntax,
                "{operator:?}"
            );
        }
    }

    #[test]
    fn text_and_image_operators_decode() {
        let n = Value::Integer(0);
        let s = Value::String(vec![]);
        for (operator, operands) in [
            (Operator::BeginText, vec![]),
            (Operator::EndText, vec![]),
            (Operator::CharacterSpacing, vec![n.clone()]),
            (Operator::WordSpacing, vec![n.clone()]),
            (Operator::HorizontalScaling, vec![n.clone()]),
            (Operator::Leading, vec![n.clone()]),
            (Operator::Rise, vec![n.clone()]),
            (Operator::RenderMode, vec![n.clone()]),
            (Operator::Font, vec![Value::name(b"F"), n.clone()]),
            (Operator::TextMatrix, vec![n.clone(); 6]),
            (Operator::MoveText, vec![n.clone(); 2]),
            (Operator::MoveTextSetLeading, vec![n.clone(); 2]),
            (Operator::NextLine, vec![]),
            (Operator::ShowText, vec![s.clone()]),
            (
                Operator::ShowTextArray,
                vec![Value::Array(vec![n.clone(), s.clone()])],
            ),
            (Operator::NextLineShowText, vec![s.clone()]),
            (Operator::NextLineShowSpaced, vec![n.clone(), n.clone(), s]),
            (Operator::GlyphWidth, vec![n.clone(); 2]),
            (Operator::GlyphBox, vec![n.clone(); 6]),
            (Operator::PaintXObject, vec![Value::name(b"I")]),
            (Operator::FillColorN, vec![n.clone(), Value::name(b"P")]),
            (Operator::StrokeColorN, vec![n; MAX_COMPONENTS]),
        ] {
            assert!(operator.decode(&operands).is_ok(), "{operator:?}");
        }
    }
}
