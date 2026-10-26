//! Leader geometry from Table 178 and Figure 80 (12.5.6.7).

use crate::values::{number, optional};
use pagura_core::{Dictionary, Error, Result};
use pagura_document::Document;

type Point = (f64, f64);

pub(super) struct Layout {
    pub(super) line: [Point; 2],
    pub(super) leaders: Option<[[Point; 2]; 2]>,
}

impl Layout {
    pub(super) fn read(
        document: &Document,
        annotation: &Dictionary,
        points: [Point; 2],
    ) -> Result<Self> {
        let length = optional(document, annotation, b"LL")?
            .map(|v| number(document, v))
            .transpose()?;
        let extension = nonnegative(document, annotation, b"LLE")?;
        let offset = nonnegative(document, annotation, b"LLO")?.unwrap_or(0.0);
        if extension.is_some() && length.is_none() {
            return Err(Error::syntax("line LLE requires LL"));
        }
        let plain = Self {
            line: points,
            leaders: None,
        };
        // Without LL, L names the line proper. LLO has no leader to offset.
        let Some(length) = length else {
            return Ok(plain);
        };
        let extension = extension.unwrap_or(0.0);
        if length == 0.0 && extension == 0.0 && offset == 0.0 {
            return Ok(plain);
        }
        let dx = points[1].0 - points[0].0;
        let dy = points[1].1 - points[0].1;
        let distance = dx.hypot(dy);
        if !distance.is_finite() {
            return Err(Error::limit("leader direction exceeds numeric range"));
        }
        // Positive LL leaders extend clockwise from the line proper, so
        // the line lies counterclockwise from its source leader endpoints.
        // A directionless line uses +x; zero LL uses the positive side.
        let (x, y) = if distance == 0.0 {
            (1.0, 0.0)
        } else {
            (dx / distance, dy / distance)
        };
        let sign = if length < 0.0 { -1.0 } else { 1.0 };
        let normal = (-y * sign, x * sign);
        let shift = |point: Point, amount: f64| {
            let point = (point.0 + normal.0 * amount, point.1 + normal.1 * amount);
            if !point.0.is_finite() || !point.1.is_finite() {
                return Err(Error::limit("leader geometry exceeds numeric range"));
            }
            Ok(point)
        };
        let height = offset + length.abs();
        let line = [shift(points[0], height)?, shift(points[1], height)?];
        let leaders = [
            [
                shift(points[0], offset)?,
                shift(points[0], height + extension)?,
            ],
            [
                shift(points[1], offset)?,
                shift(points[1], height + extension)?,
            ],
        ];
        Ok(Self {
            line,
            leaders: Some(leaders),
        })
    }
}

fn nonnegative(document: &Document, annotation: &Dictionary, key: &[u8]) -> Result<Option<f64>> {
    optional(document, annotation, key)?
        .map(|value| {
            let value = number(document, value)?;
            if value < 0.0 {
                return Err(Error::syntax("line extension/offset must be nonnegative"));
            }
            Ok(value)
        })
        .transpose()
}
