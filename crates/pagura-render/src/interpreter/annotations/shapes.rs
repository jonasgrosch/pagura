//! Generated link/square borders and note icons (12.5.4, 12.5.6.4–12.5.6.8).
//! Note icon outlines are device policy; no document font is required.
use crate::color::{Color, Space};
use crate::display::{Clip, Command, Mark, PathCommand};
use crate::interpreter::Compiler;
use pagura_core::{Dictionary, Error, Rect, Result, Value};
use pagura_raster::{FillRule, Path, PathBuilder, Point, Stroke, Transform};
use std::sync::Arc;

impl Compiler<'_> {
    pub(super) fn generated_annotation(
        &mut self,
        d: &Dictionary,
        subtype: &[u8],
        rect: Rect,
        flags: i64,
    ) -> Result<()> {
        let start = self.list.commands.len();
        match subtype {
            b"Link" | b"Square" => self.annotation_border(d, rect, subtype == b"Square")?,
            b"Text" => self.annotation_icon(d, rect)?,
            b"Widget" => self.annotation_widget(d, rect)?,
            _ => {
                return Err(Error::unsupported(
                    crate::features::ANNOTATION_APPEARANCE,
                    "annotation appearance generation is not implemented",
                ));
            }
        }
        self.annotation_view(
            start,
            rect,
            if subtype == b"Text" {
                flags | 8 | 16
            } else {
                flags
            },
        );
        Ok(())
    }
    fn annotation_border(&mut self, d: &Dictionary, rect: Rect, square: bool) -> Result<()> {
        if square && (d.contains_key(b"BE") || d.contains_key(b"RD")) {
            return Err(Error::unsupported(
                crate::features::ANNOTATION_APPEARANCE,
                "square border effects and rectangle differences are not implemented",
            ));
        }
        let Border {
            width,
            dash,
            underline,
        } = border(self.document, d)?;
        let inset = (width * 0.5)
            .min(rect.width() * 0.5)
            .min(rect.height() * 0.5);
        let inner = Rect::from_corners(
            rect.x0() + inset,
            rect.y0() + inset,
            rect.x1() - inset,
            rect.y1() - inset,
        )
        .ok_or(Error::syntax("invalid border rectangle"))?;
        if square && let Some(color) = annotation_color(self.document, d, b"IC", None)? {
            self.annotation_mark(
                d,
                rect_path(inner),
                Mark::Fill(FillRule::NonZero),
                color,
                rect,
            )?;
        }
        if width == 0.0 {
            return Ok(());
        }
        let Some(color) = annotation_color(self.document, d, b"C", Some(Color::default()))? else {
            return Ok(());
        };
        let mut path = PathBuilder::new();
        if underline {
            path.move_to(Point::new(inner.x0(), inner.y0()));
            path.line_to(Point::new(inner.x1(), inner.y0()));
        } else {
            path.rect(inner.x0(), inner.y0(), inner.width(), inner.height());
        }
        let stroke = Stroke {
            width,
            dash_array: dash,
            ..Stroke::default()
        };
        self.annotation_mark(
            d,
            path.finish(),
            Mark::Stroke(stroke, Transform::IDENTITY, false),
            color,
            rect,
        )
    }
    fn annotation_icon(&mut self, d: &Dictionary, rect: Rect) -> Result<()> {
        let name = d
            .get(b"Name")
            .map(|v| self.document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .map_or(b"Note".as_slice(), pagura_core::Name::as_bytes);
        let scale = Transform::scale(rect.width() / 16.0, rect.height() / 16.0)
            .then(Transform::translate(rect.x0(), rect.y0()));
        let (background, foreground) = icon(name);
        if let Some(color) = annotation_color(
            self.document,
            d,
            b"C",
            Some(Color::Solid {
                space: Space::Rgb,
                components: vec![1.0, 1.0, 0.0],
            }),
        )? {
            self.annotation_mark(
                d,
                background.transformed(scale),
                Mark::Fill(FillRule::NonZero),
                color,
                rect,
            )?;
        }
        self.annotation_mark(
            d,
            foreground.transformed(scale),
            Mark::Stroke(
                Stroke {
                    width: 1.0,
                    ..Stroke::default()
                },
                scale,
                false,
            ),
            Color::default(),
            rect,
        )
    }
    pub(super) fn annotation_mark(
        &mut self,
        d: &Dictionary,
        mut path: Path,
        mark: Mark,
        color: Color,
        rect: Rect,
    ) -> Result<()> {
        self.marks = self
            .marks
            .checked_add(1)
            .filter(|n| *n <= self.document.limits().display_commands)
            .ok_or(Error::limit("annotation mark budget exceeded"))?;
        let stroke_alpha = scalar(self.document, d, b"CA")?
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let alpha = if matches!(mark, Mark::Stroke(..)) {
            stroke_alpha
        } else {
            scalar(self.document, d, b"ca")?
                .unwrap_or(stroke_alpha)
                .clamp(0.0, 1.0)
        };
        if let Mark::Fill(rule) = mark {
            self.prepare_fill(&mut path, rule)?;
        }
        let mut clip = rect_path(rect);
        self.prepare_fill(&mut clip, FillRule::NonZero)?;
        self.list.commands.push(Command::Path(PathCommand {
            path: Arc::new(path),
            mark,
            color,
            clip: Some(Arc::new(Clip {
                parent: None,
                path: Arc::new(clip),
                rule: FillRule::NonZero,
            })),
            position: self.position,
            transfer: None,
            composite: crate::transparency::Composite {
                alpha,
                ..Default::default()
            },
        }));
        Ok(())
    }
}
pub(super) fn rect_path(r: Rect) -> Path {
    let mut p = PathBuilder::new();
    p.rect(r.x0(), r.y0(), r.width(), r.height());
    p.finish()
}
fn scalar(document: &pagura_document::Document, d: &Dictionary, key: &[u8]) -> Result<Option<f64>> {
    d.get(key)
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .ok_or(Error::syntax("annotation parameter must be numeric"))
        })
        .transpose()
}
fn numbers(document: &pagura_document::Document, value: &Value) -> Result<Vec<f64>> {
    document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("annotation numeric array expected"))?
        .iter()
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .ok_or(Error::syntax("annotation parameter must be numeric"))
        })
        .collect()
}
pub(super) fn annotation_color(
    document: &pagura_document::Document,
    d: &Dictionary,
    key: &[u8],
    default: Option<Color>,
) -> Result<Option<Color>> {
    let Some(v) = d.get(key) else {
        return Ok(default);
    };
    let components = numbers(document, v)?;
    let space = match components.len() {
        0 => return Ok(None),
        1 => Space::Gray,
        3 => Space::Rgb,
        4 => Space::Cmyk,
        _ => return Err(Error::syntax("invalid annotation colour")),
    };
    Ok(Some(Color::Solid { space, components }))
}
/// Seven predefined icons, specified as simple geometric symbols by device policy.
fn icon(name: &[u8]) -> (Path, Path) {
    let mut bg = PathBuilder::new();
    bg.rect(1.0, 1.0, 14.0, 14.0);
    let mut p = PathBuilder::new();
    p.rect(1.0, 1.0, 14.0, 14.0);
    let lines: &[&[(f64, f64)]] = match name {
        b"Comment" => &[
            &[(4., 11.), (12., 11.)],
            &[(4., 8.), (12., 8.)],
            &[(4., 5.), (9., 5.)],
            &[(4., 1.), (2., 0.)],
        ],
        b"Key" => &[
            &[(3., 9.), (3., 13.), (7., 13.), (7., 9.), (3., 9.)],
            &[(7., 9.), (12., 4.), (12., 2.), (14., 2.)],
        ],
        b"Help" => &[
            &[
                (4., 11.),
                (5., 13.),
                (10., 13.),
                (12., 11.),
                (11., 9.),
                (8., 7.),
                (8., 5.),
            ],
            &[(8., 3.), (8., 2.)],
        ],
        b"Paragraph" => &[
            &[
                (10., 2.),
                (10., 13.),
                (5., 13.),
                (3., 11.),
                (3., 9.),
                (5., 7.),
                (8., 7.),
            ],
            &[(8., 2.), (8., 13.)],
        ],
        b"NewParagraph" => &[
            &[
                (10., 4.),
                (10., 13.),
                (5., 13.),
                (3., 11.),
                (3., 9.),
                (5., 7.),
                (8., 7.),
            ],
            &[(8., 4.), (8., 13.)],
            &[(3., 2.), (13., 2.)],
        ],
        b"Insert" => &[&[(3., 4.), (8., 12.), (13., 4.)]],
        _ => &[
            &[(4., 12.), (12., 12.)],
            &[(4., 9.), (12., 9.)],
            &[(4., 6.), (10., 6.)],
            &[(10., 1.), (10., 4.), (15., 4.)],
        ],
    };
    for line in lines {
        for (i, &(x, y)) in line.iter().enumerate() {
            if i == 0 {
                p.move_to(Point::new(x, y));
            } else {
                p.line_to(Point::new(x, y));
            }
        }
    }
    (bg.finish(), p.finish())
}

pub(super) struct Border {
    pub(super) width: f64,
    pub(super) dash: Vec<f64>,
    pub(super) underline: bool,
}
pub(super) fn border(document: &pagura_document::Document, d: &Dictionary) -> Result<Border> {
    let (mut width, mut dash, mut style) = (1.0, Vec::new(), b"S".as_slice());
    if let Some(bs) = d.get(b"BS") {
        let bs = document.resolve_dictionary(bs)?;
        width = scalar(document, bs, b"W")?.unwrap_or(1.0);
        style = bs
            .get(b"S")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_name)
            .map_or(b"S", pagura_core::Name::as_bytes);
        if style == b"D" {
            dash = bs
                .get(b"D")
                .map(|v| numbers(document, v))
                .transpose()?
                .unwrap_or_else(|| vec![3.0]);
        }
    } else if let Some(border) = d.get(b"Border") {
        let border = document
            .resolve(border)?
            .as_array()
            .filter(|a| matches!(a.len(), 3 | 4))
            .ok_or(Error::syntax("invalid annotation Border"))?;
        let radii = [
            document.resolve(&border[0])?.as_number(),
            document.resolve(&border[1])?.as_number(),
        ];
        if radii != [Some(0.0), Some(0.0)] {
            return Err(Error::unsupported(
                crate::features::ANNOTATION_APPEARANCE,
                "rounded annotation borders are not implemented",
            ));
        }
        width = document
            .resolve(&border[2])?
            .as_number()
            .ok_or(Error::syntax("invalid annotation border width"))?;
        if let Some(array) = border.get(3) {
            dash = numbers(document, array)?;
        }
    }
    if width < 0.0 {
        return Err(Error::syntax("negative annotation border width"));
    }
    if matches!(style, b"B" | b"I") {
        return Err(Error::unsupported(
            crate::features::ANNOTATION_APPEARANCE,
            "relief annotation borders are not implemented",
        ));
    }
    Ok(Border {
        width,
        dash,
        underline: style == b"U",
    })
}
