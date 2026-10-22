//! The operators of Annex A, Table A.1.

/// An operator of Annex A, Table A.1. The standard closes the set; an
/// unknown word is a syntax error outside a compatibility section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Operator {
    /// `b`: close, fill (nonzero) and stroke the path (8.5.3).
    CloseFillStroke,
    /// `B`: fill (nonzero) and stroke the path.
    FillStroke,
    /// `b*`: close, fill (even-odd) and stroke the path.
    CloseFillStrokeEvenOdd,
    /// `B*`: fill (even-odd) and stroke the path.
    FillStrokeEvenOdd,
    /// `BDC`: begin a marked-content sequence with properties (14.6).
    BeginMarkedContentProperties,
    /// `BI`: begin an inline image (8.9.7).
    BeginInlineImage,
    /// `BMC`: begin a marked-content sequence.
    BeginMarkedContent,
    /// `BT`: begin a text object (9.4).
    BeginText,
    /// `BX`: begin a compatibility section (7.8.2).
    BeginCompatibility,
    /// `c`: append a cubic Bézier curve (8.5.2).
    CurveTo,
    /// `cm`: concatenate a matrix to the CTM (8.4.4).
    Concatenate,
    /// `CS`: set the stroking colour space (8.6.8).
    StrokeColorSpace,
    /// `cs`: set the nonstroking colour space.
    FillColorSpace,
    /// `d`: set the line dash pattern (8.4.4).
    Dash,
    /// `d0`: set a Type 3 glyph's width (9.6.4).
    GlyphWidth,
    /// `d1`: set a Type 3 glyph's width and bounding box.
    GlyphWidthBounds,
    /// `Do`: paint an XObject (8.8).
    XObject,
    /// `DP`: mark a point with properties (14.6).
    MarkedPointProperties,
    /// `EI`: end an inline image.
    EndInlineImage,
    /// `EMC`: end a marked-content sequence.
    EndMarkedContent,
    /// `ET`: end a text object.
    EndText,
    /// `EX`: end a compatibility section.
    EndCompatibility,
    /// `f`: fill the path (nonzero).
    Fill,
    /// `F`: fill the path (nonzero); obsolete spelling of f.
    FillObsolete,
    /// `f*`: fill the path (even-odd).
    FillEvenOdd,
    /// `G`: set a DeviceGray stroking colour.
    StrokeGray,
    /// `g`: set a DeviceGray nonstroking colour.
    FillGray,
    /// `gs`: set parameters from a graphics state dictionary (8.4.5).
    ExtGState,
    /// `h`: close the current subpath.
    ClosePath,
    /// `i`: set the flatness tolerance.
    Flatness,
    /// `ID`: begin inline image data.
    InlineImageData,
    /// `j`: set the line join style.
    LineJoin,
    /// `J`: set the line cap style.
    LineCap,
    /// `K`: set a DeviceCMYK stroking colour.
    StrokeCmyk,
    /// `k`: set a DeviceCMYK nonstroking colour.
    FillCmyk,
    /// `l`: append a line segment.
    LineTo,
    /// `m`: begin a new subpath.
    MoveTo,
    /// `M`: set the miter limit.
    MiterLimit,
    /// `MP`: mark a point.
    MarkedPoint,
    /// `n`: end the path without filling or stroking.
    EndPath,
    /// `q`: save the graphics state (8.4.2).
    Save,
    /// `Q`: restore the graphics state.
    Restore,
    /// `re`: append a rectangle.
    Rectangle,
    /// `RG`: set a DeviceRGB stroking colour.
    StrokeRgb,
    /// `rg`: set a DeviceRGB nonstroking colour.
    FillRgb,
    /// `ri`: set the rendering intent.
    RenderingIntent,
    /// `s`: close and stroke the path.
    CloseStroke,
    /// `S`: stroke the path.
    Stroke,
    /// `SC`: set the stroking colour.
    StrokeColor,
    /// `sc`: set the nonstroking colour.
    FillColor,
    /// `SCN`: set the stroking colour, including patterns and special spaces.
    StrokeColorN,
    /// `scn`: set the nonstroking colour, including patterns and special spaces.
    FillColorN,
    /// `sh`: paint a shading (8.7.4.3).
    Shading,
    /// `T*`: move to the start of the next text line (9.4.2).
    NextLine,
    /// `Tc`: set character spacing (9.3.2).
    CharSpacing,
    /// `Td`: move the text position.
    MoveText,
    /// `TD`: move the text position and set the leading.
    MoveTextLeading,
    /// `Tf`: set the font and size.
    Font,
    /// `Tj`: show a string (9.4.3).
    ShowText,
    /// `TJ`: show strings with individual positioning.
    ShowTextAdjusted,
    /// `TL`: set the text leading.
    Leading,
    /// `Tm`: set the text matrix and text line matrix.
    TextMatrix,
    /// `Tr`: set the text rendering mode.
    RenderMode,
    /// `Ts`: set the text rise.
    Rise,
    /// `Tw`: set word spacing.
    WordSpacing,
    /// `Tz`: set horizontal scaling.
    HorizontalScaling,
    /// `v`: append a curve with the current point as first control point.
    CurveToInitial,
    /// `w`: set the line width.
    LineWidth,
    /// `W`: intersect the clip with the path (nonzero).
    Clip,
    /// `W*`: intersect the clip with the path (even-odd).
    ClipEvenOdd,
    /// `y`: append a curve with the end point as second control point.
    CurveToFinal,
    /// `'`: move to the next line and show a string.
    NextLineShowText,
    /// `"`: set spacing, move to the next line and show a string.
    NextLineShowTextSpaced,
}

impl Operator {
    /// The operator spelled `word`, if any.
    pub(crate) fn from_bytes(word: &[u8]) -> Option<Self> {
        Some(match word {
            b"b" => Self::CloseFillStroke,
            b"B" => Self::FillStroke,
            b"b*" => Self::CloseFillStrokeEvenOdd,
            b"B*" => Self::FillStrokeEvenOdd,
            b"BDC" => Self::BeginMarkedContentProperties,
            b"BI" => Self::BeginInlineImage,
            b"BMC" => Self::BeginMarkedContent,
            b"BT" => Self::BeginText,
            b"BX" => Self::BeginCompatibility,
            b"c" => Self::CurveTo,
            b"cm" => Self::Concatenate,
            b"CS" => Self::StrokeColorSpace,
            b"cs" => Self::FillColorSpace,
            b"d" => Self::Dash,
            b"d0" => Self::GlyphWidth,
            b"d1" => Self::GlyphWidthBounds,
            b"Do" => Self::XObject,
            b"DP" => Self::MarkedPointProperties,
            b"EI" => Self::EndInlineImage,
            b"EMC" => Self::EndMarkedContent,
            b"ET" => Self::EndText,
            b"EX" => Self::EndCompatibility,
            b"f" => Self::Fill,
            b"F" => Self::FillObsolete,
            b"f*" => Self::FillEvenOdd,
            b"G" => Self::StrokeGray,
            b"g" => Self::FillGray,
            b"gs" => Self::ExtGState,
            b"h" => Self::ClosePath,
            b"i" => Self::Flatness,
            b"ID" => Self::InlineImageData,
            b"j" => Self::LineJoin,
            b"J" => Self::LineCap,
            b"K" => Self::StrokeCmyk,
            b"k" => Self::FillCmyk,
            b"l" => Self::LineTo,
            b"m" => Self::MoveTo,
            b"M" => Self::MiterLimit,
            b"MP" => Self::MarkedPoint,
            b"n" => Self::EndPath,
            b"q" => Self::Save,
            b"Q" => Self::Restore,
            b"re" => Self::Rectangle,
            b"RG" => Self::StrokeRgb,
            b"rg" => Self::FillRgb,
            b"ri" => Self::RenderingIntent,
            b"s" => Self::CloseStroke,
            b"S" => Self::Stroke,
            b"SC" => Self::StrokeColor,
            b"sc" => Self::FillColor,
            b"SCN" => Self::StrokeColorN,
            b"scn" => Self::FillColorN,
            b"sh" => Self::Shading,
            b"T*" => Self::NextLine,
            b"Tc" => Self::CharSpacing,
            b"Td" => Self::MoveText,
            b"TD" => Self::MoveTextLeading,
            b"Tf" => Self::Font,
            b"Tj" => Self::ShowText,
            b"TJ" => Self::ShowTextAdjusted,
            b"TL" => Self::Leading,
            b"Tm" => Self::TextMatrix,
            b"Tr" => Self::RenderMode,
            b"Ts" => Self::Rise,
            b"Tw" => Self::WordSpacing,
            b"Tz" => Self::HorizontalScaling,
            b"v" => Self::CurveToInitial,
            b"w" => Self::LineWidth,
            b"W" => Self::Clip,
            b"W*" => Self::ClipEvenOdd,
            b"y" => Self::CurveToFinal,
            b"'" => Self::NextLineShowText,
            b"\"" => Self::NextLineShowTextSpaced,
            _ => return None,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::Operator;
    use std::collections::HashSet;

    /// Every operator's spelling, in the order of Annex A, Table A.1.
    pub(crate) const SPELLINGS: [&str; 73] = [
        "b", "B", "b*", "B*", "BDC", "BI", "BMC", "BT", "BX", "c", "cm", "CS", "cs", "d", "d0",
        "d1", "Do", "DP", "EI", "EMC", "ET", "EX", "f", "F", "f*", "G", "g", "gs", "h", "i", "ID",
        "j", "J", "K", "k", "l", "m", "M", "MP", "n", "q", "Q", "re", "RG", "rg", "ri", "s", "S",
        "SC", "sc", "SCN", "scn", "sh", "T*", "Tc", "Td", "TD", "Tf", "Tj", "TJ", "TL", "Tm", "Tr",
        "Ts", "Tw", "Tz", "v", "w", "W", "W*", "y", "'", "\"",
    ];

    #[test]
    fn every_spelling_names_its_own_operator() {
        let operators: HashSet<Operator> = SPELLINGS
            .iter()
            .filter_map(|spelling| Operator::from_bytes(spelling.as_bytes()))
            .collect();
        assert_eq!(operators.len(), SPELLINGS.len());
        assert_eq!(Operator::from_bytes(b"xyz"), None);
    }
}
