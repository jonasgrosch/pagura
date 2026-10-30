//! The content of a generated annotation appearance: filled and stroked
//! paths in device colours under the annotation's constant opacity.

pub(super) mod rounded;

use crate::program::Program;
use crate::stream::{AppearanceStream, form_dictionary};
use crate::values::numbers;
use pagura_core::{Dictionary, Error, Name, Rect, Result, Value};
use pagura_document::Document;

/// A DeviceGray, DeviceRGB or DeviceCMYK colour by component count.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Color(Vec<f64>);

impl Color {
    /// Already validated device components from the form-style reader.
    pub(super) fn from_components(components: &[f64]) -> Self {
        Self(components.to_vec())
    }

    pub(super) fn write(&self, out: &mut Program, paint: Paint) -> Result<()> {
        write_numbers(out, &self.0)?;
        out.text(self.operator(paint))?;
        out.text("\n")
    }

    /// Black, the default border and icon colour.
    pub(super) fn black() -> Self {
        Self(vec![0.0])
    }

    /// Yellow, the default note background.
    pub(super) fn yellow() -> Self {
        Self(vec![1.0, 1.0, 0.0])
    }

    /// Red, the reader's default rubber-stamp ink.
    pub(super) fn red() -> Self {
        Self(vec![1.0, 0.0, 0.0])
    }

    /// Device-component light/shadow shades for simulated relief borders.
    /// CMYK components run toward white in the opposite direction to Gray/RGB.
    pub(super) fn relief_colors(&self) -> [Self; 2] {
        let dark: Vec<_> = self
            .0
            .iter()
            .map(|value| value.clamp(0.0, 1.0) / 2.0)
            .collect();
        let light: Vec<_> = self
            .0
            .iter()
            .map(|value| value.clamp(0.0, 1.0).midpoint(1.0))
            .collect();
        if self.0.len() == 4 {
            [Self(dark), Self(light)]
        } else {
            [Self(light), Self(dark)]
        }
    }

    fn operator(&self, paint: Paint) -> &'static str {
        match (self.0.len(), paint) {
            (1, Paint::Fill) => "g",
            (1, Paint::Stroke) => "G",
            (3, Paint::Fill) => "rg",
            (3, Paint::Stroke) => "RG",
            (_, Paint::Fill) => "k",
            (_, Paint::Stroke) => "K",
        }
    }
}

/// The colour entry `key` of `dictionary`: absent gives `default`, an
/// empty array no colour, and one, three or four numbers a device colour
/// (Table 166). Components are not clamped here; colour conversion does.
pub(super) fn color(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    default: Option<Color>,
) -> Result<Option<Color>> {
    let Some(value) = dictionary.get(key) else {
        return Ok(default);
    };
    let components = numbers(document, value)?;
    match components.len() {
        0 => Ok(None),
        1 | 3 | 4 => Ok(Some(Color(components))),
        _ => Err(Error::syntax("invalid annotation colour")),
    }
}

/// One subpath of a mark.
#[derive(Debug, Clone, Copy)]
pub(super) enum Subpath<'a> {
    /// A closed rectangle from its lower-left corner and size (`re`).
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// A rectangle with bounded elliptical corner radii.
    RoundedRect { bounds: Rect, radii: [f64; 2] },
    /// An ellipse in `bounds`, using eight tangent cubic arcs.
    Ellipse(Rect),
    /// An open polyline (`m`, then `l`).
    Polyline(&'a [(f64, f64)]),
    /// A polygon (`m`, `l`, then `h`).
    Polygon(&'a [(f64, f64)]),
    /// PDF 2.0 annotation path operators, optionally closing every subpath.
    Operators {
        commands: &'a [PathOperator],
        closed: bool,
    },
}

/// Operands of one validated annotation Path entry (Table 181).
#[derive(Debug, Clone, Copy)]
pub(super) enum PathOperator {
    Move([f64; 2]),
    Line([f64; 2]),
    Cubic([f64; 6]),
}

/// A stroke's line width and dash array, with phase 0 and otherwise the
/// initial graphics state (Table 51).
#[derive(Debug, Clone, Copy)]
pub(super) struct Pen<'a> {
    pub(super) width: f64,
    pub(super) dash: &'a [f64],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Paint {
    Fill,
    Stroke,
}

/// The ExtGState resource names that set the stroke and fill opacity.
const STROKE_ALPHA: &[u8] = b"StrokeAlpha";
const FILL_ALPHA: &[u8] = b"FillAlpha";

/// Writes marks in source order. Opacity is read when the first mark that
/// uses it is written, as a reader that draws the annotation directly
/// reads it: `CA` for every mark, `ca` for fills only.
pub(super) struct Marks<'a> {
    document: &'a Document,
    annotation: &'a Dictionary,
    content: Program,
    stroke_alpha: Option<f64>,
    fill_alpha: Option<f64>,
    states: Dictionary,
    fonts: Option<Dictionary>,
    painted: bool,
}

impl<'a> Marks<'a> {
    pub(super) fn new(document: &'a Document, annotation: &'a Dictionary) -> Self {
        Self {
            document,
            annotation,
            content: Program::new(&document.limits()),
            stroke_alpha: None,
            fill_alpha: None,
            states: Dictionary::new(),
            fonts: None,
            painted: false,
        }
    }

    /// Concatenates `matrix` to the CTM for the marks that follow (`cm`).
    pub(super) fn transform(&mut self, matrix: [f64; 6]) -> Result<()> {
        self.numbers(&matrix)?;
        self.content.text("cm\n")
    }

    /// Highlights multiply the existing page colour instead of hiding text.
    pub(super) fn multiply(&mut self) -> Result<()> {
        let mut state = Dictionary::new();
        state.insert(Name::new(b"BM"), Value::name(b"Multiply"));
        self.states
            .insert(Name::new(b"HighlightBlend"), Value::Dictionary(state));
        self.content.text("/HighlightBlend gs\n")
    }

    /// Fills `path` with the nonzero winding rule.
    pub(super) fn fill(&mut self, path: &[Subpath<'_>], color: &Color) -> Result<()> {
        self.opacity(Paint::Fill)?;
        self.color(color, Paint::Fill)?;
        self.path(path)?;
        self.painted = true;
        self.content.text("f\n")
    }

    /// Strokes `path` with `pen`.
    pub(super) fn stroke(
        &mut self,
        path: &[Subpath<'_>],
        color: &Color,
        pen: Pen<'_>,
    ) -> Result<()> {
        self.opacity(Paint::Stroke)?;
        // A dash array the stroke could not follow (8.4.3.6) fails here, not
        // when the content is drawn.
        if pen.dash.iter().any(|v| *v < 0.0)
            || !pen.dash.is_empty() && pen.dash.iter().all(|v| *v == 0.0)
        {
            return Err(Error::syntax("invalid annotation dash pattern"));
        }
        if path.iter().any(|subpath| match subpath {
            Subpath::Operators { commands, .. } => nonlinear(commands),
            Subpath::RoundedRect { radii, .. } => !radii.contains(&0.0) && !pen.dash.is_empty(),
            _ => false,
        }) {
            return Err(Error::unsupported(
                crate::features::ANNOTATION_APPEARANCE,
                "generated curved-path strokes require source-curve accuracy qualification",
            ));
        }
        self.color(color, Paint::Stroke)?;
        self.numbers(&[pen.width])?;
        self.content.text("w\n[")?;
        for (i, value) in pen.dash.iter().enumerate() {
            if i > 0 {
                self.content.text(" ")?;
            }
            self.content.text(&real(*value)?)?;
        }
        self.content.text("] 0 d\n")?;
        self.path(path)?;
        self.painted = true;
        self.content.text("S\n")
    }

    /// A filled border facet uses the border's CA, preserving the fill/text ca
    /// around it. The temporary state sets even alpha 1: an earlier background
    /// may have installed a smaller ca, so omission would incorrectly inherit it.
    pub(super) fn fill_border(&mut self, path: &[Subpath<'_>], color: &Color) -> Result<()> {
        const BORDER_ALPHA: &[u8] = b"BorderAlpha";
        let alpha = self.stroke_alpha()?;
        if !self.states.contains_key(BORDER_ALPHA) {
            let mut state = Dictionary::new();
            state.insert(Name::new(b"ca"), Value::Real(alpha));
            self.states
                .insert(Name::new(BORDER_ALPHA), Value::Dictionary(state));
        }
        self.content.text("q\n/BorderAlpha gs\n")?;
        self.color(color, Paint::Fill)?;
        self.path(path)?;
        self.content.text("f\nQ\n")?;
        self.painted = true;
        Ok(())
    }

    /// Appends a prepared text object with its private font resources.
    pub(super) fn text(&mut self, program: &[u8], fonts: Dictionary, color: &Color) -> Result<()> {
        self.opacity(Paint::Fill)?;
        self.color(color, Paint::Fill)?;
        self.content.bytes(program)?;
        self.fonts = Some(fonts);
        self.painted = true;
        Ok(())
    }

    /// The appearance: the marks in a form whose BBox is `rect`, or `None`
    /// without marks.
    pub(super) fn finish(self, rect: Rect) -> Result<Option<AppearanceStream>> {
        if !self.painted {
            return Ok(None);
        }
        let mut resources = Dictionary::new();
        if !self.states.is_empty() {
            resources.insert(Name::new(b"ExtGState"), Value::Dictionary(self.states));
        }
        if let Some(fonts) = self.fonts {
            resources.insert(Name::new(b"Font"), Value::Dictionary(fonts));
        }
        let mut dictionary = form_dictionary(rect);
        dictionary.insert(Name::new(b"Resources"), Value::Dictionary(resources));
        AppearanceStream::new(self.document, dictionary, self.content.finish()).map(Some)
    }

    /// Sets the opacity of `paint` before its first mark, unless it is 1.
    fn opacity(&mut self, paint: Paint) -> Result<()> {
        let stroke = self.stroke_alpha()?;
        if paint == Paint::Fill && self.fill_alpha.is_none() {
            let alpha = scalar(self.document, self.annotation, b"ca")?
                .unwrap_or(stroke)
                .clamp(0.0, 1.0);
            self.fill_alpha = Some(alpha);
            self.set_opacity(FILL_ALPHA, b"ca", alpha)?;
        }
        Ok(())
    }

    /// The stroke opacity, `CA`, read and set before the first mark.
    fn stroke_alpha(&mut self) -> Result<f64> {
        if let Some(alpha) = self.stroke_alpha {
            return Ok(alpha);
        }
        let alpha = scalar(self.document, self.annotation, b"CA")?
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        self.stroke_alpha = Some(alpha);
        self.set_opacity(STROKE_ALPHA, b"CA", alpha)?;
        Ok(alpha)
    }

    fn set_opacity(&mut self, name: &[u8], key: &[u8], alpha: f64) -> Result<()> {
        if alpha == 1.0 {
            return Ok(());
        }
        let mut state = Dictionary::new();
        state.insert(Name::new(key), Value::Real(alpha));
        self.states
            .insert(Name::new(name), Value::Dictionary(state));
        self.content.text(&format!("{} gs\n", Name::new(name)))
    }

    fn color(&mut self, color: &Color, paint: Paint) -> Result<()> {
        color.write(&mut self.content, paint)
    }

    fn path(&mut self, path: &[Subpath<'_>]) -> Result<()> {
        for subpath in path {
            match *subpath {
                Subpath::Rect {
                    x,
                    y,
                    width,
                    height,
                } => {
                    self.numbers(&[x, y, width, height])?;
                    self.content.text("re\n")?;
                }
                Subpath::RoundedRect { bounds, radii } => {
                    rounded::write(&mut self.content, bounds, radii)?;
                }
                Subpath::Ellipse(bounds) => self.ellipse(bounds)?,
                Subpath::Polyline(points) | Subpath::Polygon(points) => {
                    write_polyline(
                        &mut self.content,
                        points,
                        matches!(subpath, Subpath::Polygon(_)),
                    )?;
                }
                Subpath::Operators { commands, closed } => {
                    for (index, command) in commands.iter().enumerate() {
                        if closed && index != 0 && matches!(command, PathOperator::Move(_)) {
                            self.content.text("h\n")?;
                        }
                        let (values, operator): (&[f64], &str) = match command {
                            PathOperator::Move(values) => (values, "m\n"),
                            PathOperator::Line(values) => (values, "l\n"),
                            PathOperator::Cubic(values) => (values, "c\n"),
                        };
                        self.numbers(values)?;
                        self.content.text(operator)?;
                    }
                    if closed {
                        self.content.text("h\n")?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Eight 45-degree arcs, with control distance 4/3*tan(pi/16).
    /// The unit-circle tangent construction is scaled independently on each
    /// axis. This bounded approximation is a reader artwork policy, not a
    /// claim that cubic Beziers represent an exact conic.
    fn ellipse(&mut self, bounds: Rect) -> Result<()> {
        use core::f64::consts::FRAC_1_SQRT_2 as Q;
        const K: f64 = 0.265_216_489_839_544;
        const POINTS: [(f64, f64); 9] = [
            (1.0, 0.0),
            (Q, Q),
            (0.0, 1.0),
            (-Q, Q),
            (-1.0, 0.0),
            (-Q, -Q),
            (0.0, -1.0),
            (Q, -Q),
            (1.0, 0.0),
        ];
        let rx = bounds.width() * 0.5;
        let ry = bounds.height() * 0.5;
        let cx = bounds.x0() + rx;
        let cy = bounds.y0() + ry;
        self.numbers(&[bounds.x1(), cy])?;
        self.content.text("m\n")?;
        for ((x0, y0), (x1, y1)) in POINTS.iter().copied().zip(POINTS.iter().copied().skip(1)) {
            self.numbers(&[
                cx + rx * (x0 - K * y0),
                cy + ry * (y0 + K * x0),
                cx + rx * (x1 + K * y1),
                cy + ry * (y1 - K * x1),
                cx + rx * x1,
                cy + ry * y1,
            ])?;
            self.content.text("c\n")?;
        }
        self.content.text("h\n")
    }

    /// Writes each value followed by a space.
    fn numbers(&mut self, values: &[f64]) -> Result<()> {
        write_numbers(&mut self.content, values)
    }
}

pub(super) fn write_numbers(out: &mut Program, values: &[f64]) -> Result<()> {
    for value in values {
        out.text(&real(*value)?)?;
        out.text(" ")?;
    }
    Ok(())
}

pub(super) fn write_polyline(out: &mut Program, points: &[(f64, f64)], closed: bool) -> Result<()> {
    for (index, &(x, y)) in points.iter().enumerate() {
        write_numbers(out, &[x, y])?;
        out.text(if index == 0 { "m\n" } else { "l\n" })?;
    }
    if closed {
        out.text("h\n")?;
    }
    Ok(())
}

/// The shortest decimal that reads back as `value` (7.3.3). A whole number
/// of more than 15 digits ends in a period, so that it reads as a real
/// rather than an integer.
fn real(value: f64) -> Result<String> {
    if !value.is_finite() {
        return Err(Error::limit("annotation appearance exceeds numeric range"));
    }
    let mut text = value.to_string();
    if !text.contains('.') && text.trim_start_matches('-').len() > 15 {
        text.push('.');
    }
    Ok(text)
}

/// A numeric annotation entry such as `CA`.
pub(super) fn scalar(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<f64>> {
    dictionary
        .get(key)
        .map(|value| {
            document
                .resolve(value)?
                .as_number()
                .ok_or(Error::syntax("annotation parameter must be numeric"))
        })
        .transpose()
}

/// Straight, ordered cubic controls retain the ordinary line-stroke path.
/// Other cubic paths need the separately qualified source-tangent/dash work.
fn nonlinear(commands: &[PathOperator]) -> bool {
    let mut current = [0.0, 0.0];
    for command in commands {
        match *command {
            PathOperator::Move(point) | PathOperator::Line(point) => current = point,
            PathOperator::Cubic([x1, y1, x2, y2, x3, y3]) => {
                let end = [x3, y3];
                let delta = [x3 - current[0], y3 - current[1]];
                let axis = usize::from(delta[1].abs() > delta[0].abs());
                let mut previous = 0.0;
                for control in [[x1, y1], [x2, y2]] {
                    if delta == [0.0, 0.0] {
                        if control != current {
                            return true;
                        }
                    } else {
                        let t = (control[axis] - current[axis]) / delta[axis];
                        if !t.is_finite()
                            || t < previous
                            || t > 1.0
                            || control[1 - axis] - current[1 - axis] != t * delta[1 - axis]
                        {
                            return true;
                        }
                        previous = t;
                    }
                }
                current = end;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::real;

    #[test]
    fn reals_read_back_exactly_in_pdf_syntax() {
        assert_eq!(real(0.5).unwrap(), "0.5");
        assert_eq!(real(-3.0).unwrap(), "-3");
        assert_eq!(real(1e20).unwrap(), "100000000000000000000.");
        assert_eq!(real(0.1 + 0.2).unwrap(), "0.30000000000000004");
        assert!(real(f64::INFINITY).is_err());
    }
}
