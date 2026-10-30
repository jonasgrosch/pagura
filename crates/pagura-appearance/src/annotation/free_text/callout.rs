//! FreeText CL/LE, Table 177 and Figure 79.

use super::super::border::Border;
use super::super::ending::{Endpoint, Style};
use super::super::marks::{Color, Marks, Pen, Subpath};
use crate::features;
use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Result};
use pagura_document::Document;

type Point = (f64, f64);

pub(super) struct Callout {
    points: [Point; 3],
    count: usize,
    ending: Style,
    endpoint: Endpoint,
}

impl Callout {
    /// CL is meaningful only for FreeTextCallout; unused CL/LE are not resolved.
    pub(super) fn read(document: &Document, annotation: &Dictionary) -> Result<Option<Self>> {
        let Some(intent) = optional(document, annotation, b"IT")? else {
            return Ok(None);
        };
        match intent
            .as_name()
            .ok_or(Error::syntax("free text IT must be a name"))?
            .as_bytes()
        {
            b"FreeText" | b"FreeTextTypeWriter" => return Ok(None),
            b"FreeTextCallout" => {}
            _ => {
                return Err(Error::unsupported(
                    features::ANNOTATION_APPEARANCE,
                    "unknown free text intent requires a stored appearance",
                ));
            }
        }
        let Some(value) = optional(document, annotation, b"CL")? else {
            return Ok(None);
        };
        let values = value
            .as_array()
            .ok_or(Error::syntax("free text CL must be an array"))?;
        if !matches!(values.len(), 4 | 6) {
            return Err(Error::syntax("free text CL must have four or six numbers"));
        }
        let mut points = [(0.0, 0.0); 3];
        let (pairs, _) = values.as_chunks::<2>();
        for (point, [x, y]) in points.iter_mut().zip(pairs) {
            *point = (number(document, x)?, number(document, y)?);
        }
        let count = pairs.len();
        let adjacent = points
            .get(1..count)
            .and_then(|rest| rest.iter().find(|point| **point != points[0]))
            .copied()
            .unwrap_or(points[0]);
        let endpoint = Endpoint::new(points[0], adjacent)?;
        // Validate every body difference even when no stroke would be drawn.
        for pair in points.get(..count).unwrap_or_default().windows(2) {
            if let [first, second] = pair {
                Endpoint::new(*first, *second)?;
            }
        }
        let ending = optional(document, annotation, b"LE")?
            .map(|value| Style::read(document, value))
            .transpose()?
            .unwrap_or(Style::None);
        Ok(Some(Self {
            points,
            count,
            ending,
            endpoint,
        }))
    }

    /// Draw before the text box, so its background hides any line continuation.
    pub(super) fn draw(
        self,
        marks: &mut Marks<'_>,
        border: &Border,
        background: Option<&Color>,
    ) -> Result<()> {
        if border.width > 0.0 {
            let points = self
                .points
                .get(..self.count)
                .ok_or(Error::limit("invalid callout length"))?;
            marks.stroke(
                &[Subpath::Polyline(points)],
                &Color::black(),
                Pen {
                    width: border.width,
                    dash: &border.dash,
                },
            )?;
        }
        self.ending
            .draw_callout(marks, self.endpoint, border, background)
    }
}
