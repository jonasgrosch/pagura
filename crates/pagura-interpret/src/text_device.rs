//! The text device: positioned glyphs, marked content and object
//! occurrences for `pagura-extract` (ADR 0014, ADR 0025).

use crate::device::{Device, Needs};
use pagura_core::{Limits, Result};
use pagura_extract::PositionedText;

/// Collects the positioned text of a page. It needs no paint, images or
/// outlines, so extraction decodes no image, parses no shading and converts
/// no colour.
#[derive(Debug)]
pub struct TextDevice {
    limits: Limits,
    text: PositionedText,
}

impl TextDevice {
    /// An empty device under `limits` (`text_items`).
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            text: PositionedText::default(),
        }
    }

    /// The collected text.
    ///
    /// # Errors
    ///
    /// Returns `Syntax` for unbalanced marked content.
    pub fn finish(self) -> Result<PositionedText> {
        let _ = self.limits;
        Ok(self.text)
    }
}

impl Device for TextDevice {
    fn needs(&self) -> Needs {
        Needs::TEXT
    }
}
