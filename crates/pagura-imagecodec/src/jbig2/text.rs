//! Arithmetic text regions and aggregation (T.88 6.4 and 7.4.3).
pub(super) mod huffman;

use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    bitmap::{CombinationOp, Fill, charge_pixels},
    generic::Prediction,
    integer::{Integer, symbol_id},
    refinement::{self, Model, Parameters, RefinementTemplate},
};
use crate::budget::Budget;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};
use std::rc::Rc;

/// Decodes an arithmetic text region segment's data after its `flags`
/// (T.88 7.4.3).
pub(super) fn decode(
    cursor: &mut Cursor<'_>,
    flags: TextFlags,
    size: (u32, u32),
    symbols: &[Rc<Bitmap>],
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Bitmap> {
    let parameters = flags
        .refinement()
        .map(|template| Parameters::read(cursor, template))
        .transpose()?;
    let instances = u32_to_usize(cursor.u32()?);
    let mut refinement = parameters
        .map(|p| Model::new(p, None, budget).map(Refinement::new))
        .transpose()?;
    let mut state = State::new(symbols.len(), budget)?;
    let mut decoder = Decoder::new(cursor)?;
    let bitmap = state.decode(
        &mut decoder,
        symbols,
        Placement {
            size,
            instances,
            config: flags.config(),
        },
        refinement.as_mut(),
        limits,
        budget,
    )?;
    state.release(budget);
    if let Some(refinement) = refinement {
        refinement.model.release(budget);
    }
    Ok(bitmap)
}

/// Text region segment flags (T.88 7.4.3.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TextFlags(u16);

impl TextFlags {
    /// Bit 0, SBHUFF: Huffman coding.
    const HUFFMAN: u16 = 0x0001;
    /// Bit 1, SBREFINE: instances may be refined.
    const REFINE: u16 = 0x0002;
    /// Bits 2–3, LOGSBSTRIPS: the base-2 logarithm of the strip size.
    const LOG_STRIPS_SHIFT: u32 = 2;
    /// Bits 4–5, REFCORNER.
    const CORNER_SHIFT: u32 = 4;
    /// Bit 6, TRANSPOSED.
    const TRANSPOSED: u16 = 0x0040;
    /// Bits 7–8, SBCOMBOP.
    const OPERATOR_SHIFT: u32 = 7;
    /// Bit 9, SBDEFPIXEL.
    const DEFAULT_PIXEL: u16 = 0x0200;
    /// Bits 10–14, SBDSOFFSET: a signed five-bit offset.
    const OFFSET_SHIFT: u32 = 10;
    /// Bit 15, SBRTEMPLATE.
    const REFINEMENT_TEMPLATE: u16 = 0x8000;

    /// Reads the flags, which select a refinement template only for refined
    /// text.
    pub(super) fn read(cursor: &mut Cursor<'_>) -> Result<Self> {
        let flags = Self(cursor.u16()?);
        if flags.0 & Self::REFINE == 0 && flags.0 & Self::REFINEMENT_TEMPLATE != 0 {
            return Err(Error::syntax("unused JBIG2 refinement template flag"));
        }
        Ok(flags)
    }

    pub(super) const fn huffman(self) -> bool {
        self.0 & Self::HUFFMAN != 0
    }

    /// The refinement template of refined text.
    pub(super) const fn refinement(self) -> Option<RefinementTemplate> {
        if self.0 & Self::REFINE == 0 {
            None
        } else {
            Some(RefinementTemplate::from_flag(
                self.0 & Self::REFINEMENT_TEMPLATE != 0,
            ))
        }
    }

    /// How the region places its instances.
    pub(super) fn config(self) -> Config {
        let offset = i64::from((self.0 >> Self::OFFSET_SHIFT) & 0x1f);
        Config {
            strips: 1 << ((self.0 >> Self::LOG_STRIPS_SHIFT) & 3),
            operator: CombinationOp::from_two_bits(self.0 >> Self::OPERATOR_SHIFT),
            offset: if offset >= 16 { offset - 32 } else { offset },
            fill: Fill::from_flag(self.0 & Self::DEFAULT_PIXEL != 0),
            corner: ReferenceCorner::from_two_bits(self.0 >> Self::CORNER_SHIFT),
            transposed: self.0 & Self::TRANSPOSED != 0,
            complete_last_strip: false,
        }
    }
}

/// The corner of a symbol placed at an instance's coordinates, REFCORNER
/// (T.88 7.4.3.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceCorner {
    BottomLeft,
    TopLeft,
    BottomRight,
    TopRight,
}

impl ReferenceCorner {
    const fn from_two_bits(bits: u16) -> Self {
        match bits & 3 {
            0 => Self::BottomLeft,
            1 => Self::TopLeft,
            2 => Self::BottomRight,
            _ => Self::TopRight,
        }
    }

    const fn is_bottom(self) -> bool {
        matches!(self, Self::BottomLeft | Self::BottomRight)
    }

    const fn is_right(self) -> bool {
        matches!(self, Self::BottomRight | Self::TopRight)
    }
}

/// The integer and symbol identifier contexts of text decoding (T.88
/// 6.4.5).
pub(super) struct State {
    identifiers: Vec<Context>,
    delta_t: Integer,
    first_s: Integer,
    delta_s: Integer,
    current_t: Integer,
}

/// The refinement model of refined text and its integer contexts.
pub(super) struct Refinement {
    model: Model,
    indicator: Integer,
    width: Integer,
    height: Integer,
    x: Integer,
    y: Integer,
}

pub(super) struct Placement {
    pub(super) size: (u32, u32),
    pub(super) instances: usize,
    pub(super) config: Config,
}

/// Refinement and aggregate coding of an arithmetic symbol dictionary
/// (T.88 6.5.8.2), which always refines.
pub(super) struct Aggregation {
    state: State,
    refinement: Refinement,
}

impl Aggregation {
    pub(super) fn new(symbol_count: usize, model: Model, budget: &mut Budget) -> Result<Self> {
        Ok(Self {
            state: State::new(symbol_count, budget)?,
            refinement: Refinement::new(model),
        })
    }

    /// The refinement contexts, for a later dictionary to reuse; the other
    /// contexts are released.
    pub(super) fn into_refinement_contexts(self, budget: &mut Budget) -> Vec<Context> {
        self.state.release(budget);
        self.refinement.model.contexts
    }

    /// Drops the aggregation state, releasing its contexts.
    pub(super) fn release(self, budget: &mut Budget) {
        self.state.release(budget);
        self.refinement.model.release(budget);
    }

    /// Decodes an aggregate of several symbol instances.
    pub(super) fn decode(
        &mut self,
        decoder: &mut Decoder<'_>,
        symbols: &[Rc<Bitmap>],
        placement: Placement,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        self.state.decode(
            decoder,
            symbols,
            placement,
            Some(&mut self.refinement),
            limits,
            budget,
        )
    }

    /// Decodes a refinement of one symbol (T.88 6.5.8.2.2).
    pub(super) fn single(
        &mut self,
        decoder: &mut Decoder<'_>,
        symbols: &[Rc<Bitmap>],
        size: (u32, u32),
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        let id = symbol_id(decoder, &mut self.state.identifiers);
        let symbol = symbols
            .get(id)
            .ok_or_else(|| Error::syntax("JBIG2 refinement symbol identifier out of range"))?;
        let refinement = &mut self.refinement;
        let dx = refinement.x.required(decoder)?;
        let dy = refinement.y.required(decoder)?;
        refinement.model.decode(
            decoder,
            symbol,
            refinement::Request {
                width: size.0,
                height: size.1,
                dx,
                dy,
                prediction: Prediction::Off,
            },
            limits,
            budget,
        )
    }
}

impl Refinement {
    fn new(model: Model) -> Self {
        Self {
            model,
            indicator: Integer::new(),
            width: Integer::new(),
            height: Integer::new(),
            x: Integer::new(),
            y: Integer::new(),
        }
    }

    /// The refinement of an instance of `symbol`, if its indicator asks for
    /// one (T.88 6.4.11).
    fn refine(
        &mut self,
        decoder: &mut Decoder<'_>,
        symbol: &Bitmap,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Option<Bitmap>> {
        match self.indicator.required(decoder)? {
            0 => return Ok(None),
            1 => {}
            _ => return Err(Error::syntax("invalid JBIG2 text refinement indicator")),
        }
        let dw = self.width.required(decoder)?;
        let dh = self.height.required(decoder)?;
        let width = dimension(symbol.width, dw)?;
        let height = dimension(symbol.height, dh)?;
        let dx = (dw >> 1) + self.x.required(decoder)?;
        let dy = (dh >> 1) + self.y.required(decoder)?;
        self.model
            .decode(
                decoder,
                symbol,
                refinement::Request {
                    width,
                    height,
                    dx,
                    dy,
                    prediction: Prediction::Off,
                },
                limits,
                budget,
            )
            .map(Some)
    }
}

impl State {
    /// Drops the state, releasing its identifier contexts.
    fn release(self, budget: &mut Budget) {
        budget.release::<Context>(self.identifiers.len());
    }

    /// The decoding state for `symbol_count` symbols, whose identifier
    /// contexts it allocates.
    pub(super) fn new(symbol_count: usize, budget: &mut Budget) -> Result<Self> {
        let count = symbol_count
            .max(1)
            .checked_next_power_of_two()
            .ok_or_else(|| Error::limit("JBIG2 identifier context count overflow"))?;
        Ok(Self {
            identifiers: budget.filled(count, Context::default())?,
            delta_t: Integer::new(),
            first_s: Integer::new(),
            delta_s: Integer::new(),
            current_t: Integer::new(),
        })
    }

    /// Decodes the instances of a text region or aggregate (T.88 6.4.5),
    /// refining them with `refinement` if given.
    pub(super) fn decode(
        &mut self,
        decoder: &mut Decoder<'_>,
        symbols: &[Rc<Bitmap>],
        placement: Placement,
        mut refinement: Option<&mut Refinement>,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Bitmap> {
        let Placement {
            size: (width, height),
            instances,
            config,
        } = placement;
        budget.work(instances)?;
        if instances != 0 && symbols.is_empty() {
            return Err(Error::syntax("JBIG2 text region has no symbols"));
        }
        let mut bitmap = Bitmap::new(width, height, config.fill, limits, budget)?;
        let mut strip_t = -self.delta_t.required(decoder)? * config.strips;
        let mut first = 0;
        let mut count = 0;
        while count < instances {
            strip_t = add(strip_t, self.delta_t.required(decoder)? * config.strips)?;
            first = add(first, self.first_s.required(decoder)?)?;
            let mut s = first;
            loop {
                let t = if config.strips == 1 {
                    0
                } else {
                    self.current_t.required(decoder)?
                };
                if t < 0 || t >= config.strips {
                    return Err(Error::syntax("invalid JBIG2 text position within strip"));
                }
                let t = add(strip_t, t)?;
                let id = symbol_id(decoder, &mut self.identifiers);
                let symbol = symbols
                    .get(id)
                    .ok_or_else(|| Error::syntax("JBIG2 symbol identifier out of range"))?;
                let refined = match refinement.as_deref_mut() {
                    Some(refinement) => refinement.refine(decoder, symbol, limits, budget)?,
                    None => None,
                };
                let glyph = refined.as_ref().unwrap_or(symbol);
                charge_pixels(budget, u64::from(glyph.width) * u64::from(glyph.height))?;
                let (x, y) = config.origin(s, t, glyph)?;
                bitmap.combine_signed(glyph, x, y, config.operator);
                let advance = config.advance(glyph);
                if let Some(refined) = refined {
                    refined.release(budget);
                }
                count += 1;
                if count == instances && !config.complete_last_strip {
                    break;
                }
                s = add(s, advance)?;
                let Some(delta) = self.delta_s.decode(decoder) else {
                    break;
                };
                if count == instances {
                    return Err(Error::syntax(
                        "JBIG2 aggregate strip lacks final out-of-band",
                    ));
                }
                s = add(s, delta + config.offset)?;
            }
        }
        Ok(bitmap)
    }
}

/// A refined symbol's width or height: the symbol's plus a decoded
/// difference, which must leave it positive (T.88 6.4.11).
fn dimension(base: u32, delta: i64) -> Result<u32> {
    u32::try_from(i64::from(base) + delta)
        .ok()
        .filter(|&n| n != 0)
        .ok_or_else(|| Error::syntax("invalid JBIG2 refined symbol dimension"))
}

/// How a text region places its symbol instances (T.88 6.4.5).
pub(super) struct Config {
    pub(super) strips: i64,
    pub(super) operator: CombinationOp,
    pub(super) offset: i64,
    pub(super) fill: Fill,
    corner: ReferenceCorner,
    transposed: bool,
    complete_last_strip: bool,
}

impl Config {
    /// The placement of a symbol dictionary's aggregates (T.88 6.5.8.2.2).
    pub(super) const fn aggregation() -> Self {
        Self {
            strips: 1,
            operator: CombinationOp::Or,
            offset: 0,
            fill: Fill::White,
            corner: ReferenceCorner::TopLeft,
            transposed: false,
            complete_last_strip: true,
        }
    }

    /// T.88 6.4.5 steps 3 c) vi–x: a right/bottom corner on the S axis
    /// advances CURS before placement, cancelling that corner's subtraction.
    /// The other corners advance it afterwards. Thus S always denotes the
    /// leading pixel here, and every corner advances by the coding-axis size.
    pub(super) fn origin(&self, s: i64, t: i64, glyph: &Bitmap) -> Result<(i64, i64)> {
        if self.transposed {
            let shift = if self.corner.is_right() {
                1 - i64::from(glyph.width)
            } else {
                0
            };
            Ok((add(t, shift)?, s))
        } else {
            let shift = if self.corner.is_bottom() {
                1 - i64::from(glyph.height)
            } else {
                0
            };
            Ok((s, add(t, shift)?))
        }
    }

    pub(super) fn advance(&self, glyph: &Bitmap) -> i64 {
        i64::from(if self.transposed {
            glyph.height
        } else {
            glyph.width
        }) - 1
    }
}

pub(super) fn add(left: i64, right: i64) -> Result<i64> {
    left.checked_add(right)
        .ok_or_else(|| Error::limit("JBIG2 text coordinate overflow"))
}
