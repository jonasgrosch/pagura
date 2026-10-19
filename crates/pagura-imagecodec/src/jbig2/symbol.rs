//! Arithmetic symbol dictionaries and retained contexts (T.88 6.5 and
//! 7.4.2).
pub(super) mod huffman;

use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    bitmap::{self, Fill, charge_pixels},
    generic::{self, GenericTemplate, Prediction, Template},
    integer::Integer,
    refinement::{Model, Parameters, RefinementTemplate},
    text,
};
use crate::budget::Budget;
use pagura_core::bytes::Cursor;
use pagura_core::convert::u32_to_usize;
use pagura_core::{Error, Limits, Result};
use std::rc::Rc;

pub(super) struct Dictionary {
    pub(super) symbols: Vec<Rc<Bitmap>>,
    pub(super) contexts: Option<Retained>,
}

pub(super) struct Retained {
    pub(super) settings: Settings,
    pub(super) generic: Vec<Context>,
    pub(super) refinement: Option<Vec<Context>>,
}

struct Config {
    settings: Settings,
    generic: Template,
    refinement: Option<Parameters>,
    reuse: bool,
    retain: bool,
}

struct Procedure {
    heights: Integer,
    widths: Integer,
    instances: Integer,
    generic: Vec<Context>,
    aggregation: Option<text::Aggregation>,
}

/// Symbol dictionary segment flags (T.88 7.4.2.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SymbolFlags(u16);

impl SymbolFlags {
    /// Bit 0, SDHUFF: Huffman coding.
    const HUFFMAN: u16 = 0x0001;
    /// Bit 1, SDREFAGG: refinement and aggregate coding.
    const AGGREGATE: u16 = 0x0002;
    /// Bits 2–3, SDHUFFDH: the height difference table.
    const HEIGHT_TABLE_SHIFT: u32 = 2;
    /// Bits 4–5, SDHUFFDW: the width difference table.
    const WIDTH_TABLE_SHIFT: u32 = 4;
    /// Bit 6, SDHUFFBMSIZE: a user table for collective bitmap sizes.
    const SIZE_TABLE: u16 = 0x0040;
    /// Bit 7, SDHUFFAGGINST: a user table for aggregate instance counts.
    const INSTANCE_TABLE: u16 = 0x0080;
    /// Bits 2–7: the Huffman table selections.
    const TABLES: u16 = 0x00fc;
    /// Bit 8: the coding contexts retained by the last dictionary are used.
    const CONTEXTS_USED: u16 = 0x0100;
    /// Bit 9: the coding contexts are retained for a later dictionary.
    const CONTEXTS_RETAINED: u16 = 0x0200;
    /// Bits 10–11, SDTEMPLATE.
    const TEMPLATE: u16 = 0x0c00;
    const TEMPLATE_SHIFT: u32 = 10;
    /// Bit 12, SDRTEMPLATE.
    const REFINEMENT_TEMPLATE: u16 = 0x1000;
    /// Bits 13–15: reserved.
    const RESERVED: u16 = 0xe000;
    /// The fields that decide how bitmaps are coded.
    const CODING: u16 =
        Self::HUFFMAN | Self::AGGREGATE | Self::TEMPLATE | Self::REFINEMENT_TEMPLATE;

    pub(super) fn read(cursor: &mut Cursor<'_>) -> Result<Self> {
        cursor.u16().map(Self)
    }

    pub(super) const fn huffman(self) -> bool {
        self.0 & Self::HUFFMAN != 0
    }

    const fn aggregate(self) -> bool {
        self.0 & Self::AGGREGATE != 0
    }

    const fn contexts_used(self) -> bool {
        self.0 & Self::CONTEXTS_USED != 0
    }

    const fn contexts_retained(self) -> bool {
        self.0 & Self::CONTEXTS_RETAINED != 0
    }

    const fn template(self) -> GenericTemplate {
        GenericTemplate::from_two_bits(self.0 >> Self::TEMPLATE_SHIFT)
    }

    /// The refinement template of a dictionary with refinement and
    /// aggregate coding.
    const fn refinement(self) -> Option<RefinementTemplate> {
        if self.aggregate() {
            Some(RefinementTemplate::from_flag(
                self.0 & Self::REFINEMENT_TEMPLATE != 0,
            ))
        } else {
            None
        }
    }
}

/// Decodes an arithmetic symbol dictionary segment's data after its
/// `flags` (T.88 7.4.2).
pub(super) fn decode(
    cursor: &mut Cursor<'_>,
    flags: SymbolFlags,
    imported: &[Rc<Bitmap>],
    retained: Option<&Retained>,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Dictionary> {
    let config = Config::read(cursor, flags)?;
    let exported = u32_to_usize(cursor.u32()?);
    let added = u32_to_usize(cursor.u32()?);
    let total = imported
        .len()
        .checked_add(added)
        .ok_or_else(|| Error::limit("JBIG2 symbol count overflow"))?;
    if exported > total {
        return Err(Error::syntax(
            "JBIG2 exported symbol count exceeds dictionary",
        ));
    }
    let mut symbols = Vec::new();
    budget.reserve(&mut symbols, total)?;
    symbols.extend(imported.iter().cloned());
    let mut procedure = Procedure::new(&config, retained, total, budget)?;
    let mut decoder = Decoder::new(cursor)?;
    procedure.new_symbols(&mut decoder, &config, &mut symbols, total, limits, budget)?;
    let selected = select(&symbols, exported, &mut decoder, budget)?;
    release_symbols(symbols, budget);
    let contexts = if config.retain {
        Some(Retained {
            settings: config.settings,
            generic: procedure.generic,
            refinement: procedure
                .aggregation
                .map(|aggregation| aggregation.into_refinement_contexts(budget)),
        })
    } else {
        budget.release::<Context>(procedure.generic.len());
        if let Some(aggregation) = procedure.aggregation {
            aggregation.release(budget);
        }
        None
    };
    Ok(Dictionary {
        symbols: selected,
        contexts,
    })
}

/// Drops the symbol table of a decoded dictionary, releasing it and the new
/// symbols that the dictionary does not export.
fn release_symbols(symbols: Vec<Rc<Bitmap>>, budget: &mut Budget) {
    budget.release::<Rc<Bitmap>>(symbols.len());
    for symbol in symbols {
        bitmap::unshare(symbol, budget);
    }
}

/// The bitmap coding parameters of a dictionary, which must all match when
/// a later dictionary reuses its coding contexts (T.88 7.4.2.2 step 3).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Settings {
    coding: u16,
    generic: Option<Template>,
    refinement: Option<Parameters>,
}

impl Settings {
    pub(super) const fn new(
        flags: SymbolFlags,
        generic: Option<Template>,
        refinement: Option<Parameters>,
    ) -> Self {
        Self {
            coding: flags.0 & SymbolFlags::CODING,
            generic,
            refinement,
        }
    }

    pub(super) fn check(self, retained: &Retained) -> Result<()> {
        if self != retained.settings {
            return Err(Error::syntax(
                "incompatible JBIG2 retained bitmap parameters",
            ));
        }
        Ok(())
    }
}

impl Config {
    fn read(cursor: &mut Cursor<'_>, flags: SymbolFlags) -> Result<Self> {
        debug_assert!(!flags.huffman(), "Huffman symbol dictionary");
        if flags.0 & SymbolFlags::TABLES != 0
            || !flags.aggregate() && flags.0 & SymbolFlags::REFINEMENT_TEMPLATE != 0
        {
            return Err(Error::syntax("unused JBIG2 symbol coding flags"));
        }
        if flags.0 & SymbolFlags::RESERVED != 0 {
            return Err(Error::syntax("reserved JBIG2 symbol dictionary flags"));
        }
        let generic = Template::read(cursor, flags.template())?;
        let refinement = flags
            .refinement()
            .map(|template| Parameters::read(cursor, template))
            .transpose()?;
        Ok(Self {
            settings: Settings::new(flags, Some(generic), refinement),
            generic,
            refinement,
            reuse: flags.contexts_used(),
            retain: flags.contexts_retained(),
        })
    }
}

impl Procedure {
    fn new(
        config: &Config,
        retained: Option<&Retained>,
        total: usize,
        budget: &mut Budget,
    ) -> Result<Self> {
        let retained = if config.reuse {
            Some(retained.ok_or_else(|| Error::syntax("missing JBIG2 retained symbol contexts"))?)
        } else {
            None
        };
        let count = config.generic.context_count();
        if retained.is_some_and(|retained| retained.generic.len() != count) {
            return Err(Error::syntax(
                "incompatible JBIG2 retained generic contexts",
            ));
        }
        let mut generic = budget.filled(count, Context::default())?;
        if let Some(retained) = retained {
            generic.copy_from_slice(&retained.generic);
        }
        let aggregation = if let Some(parameters) = config.refinement {
            let previous = retained.and_then(|r| r.refinement.as_deref());
            if config.reuse && previous.is_none() {
                return Err(Error::syntax("missing JBIG2 retained refinement contexts"));
            }
            let model = Model::new(parameters, previous, budget)?;
            Some(text::Aggregation::new(total, model, budget)?)
        } else {
            None
        };
        if let Some(retained) = retained {
            config.settings.check(retained)?;
        }
        Ok(Self {
            heights: Integer::new(),
            widths: Integer::new(),
            instances: Integer::new(),
            generic,
            aggregation,
        })
    }

    fn new_symbols(
        &mut self,
        decoder: &mut Decoder<'_>,
        config: &Config,
        symbols: &mut Vec<Rc<Bitmap>>,
        total: usize,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<()> {
        let mut height = 0i64;
        while symbols.len() < total {
            budget.work(1)?;
            height += self.heights.required(decoder)?;
            let height = positive(height, "invalid JBIG2 symbol height")?;
            let mut width = 0i64;
            while let Some(delta) = self.widths.decode(decoder) {
                if symbols.len() == total {
                    return Err(Error::syntax("too many JBIG2 new symbols"));
                }
                width += delta;
                let width = positive(width, "invalid JBIG2 symbol width")?;
                bitmap::layout(width, height, limits)?;
                charge_pixels(budget, u64::from(width) * u64::from(height))?;
                let bitmap = if let Some(aggregation) = &mut self.aggregation {
                    let instances = usize::try_from(self.instances.required(decoder)?)
                        .ok()
                        .filter(|&n| n != 0)
                        .ok_or_else(|| Error::syntax("invalid JBIG2 aggregate instance count"))?;
                    if instances == 1 {
                        aggregation.single(decoder, symbols, (width, height), limits, budget)?
                    } else {
                        aggregation.decode(
                            decoder,
                            symbols,
                            text::Placement {
                                size: (width, height),
                                instances,
                                config: text::Config::aggregation(),
                            },
                            limits,
                            budget,
                        )?
                    }
                } else {
                    let mut bitmap = Bitmap::new(width, height, Fill::White, limits, budget)?;
                    generic::decode_into(
                        &mut bitmap,
                        &config.generic,
                        Prediction::Off,
                        decoder,
                        &mut self.generic,
                        None,
                    );
                    bitmap
                };
                symbols.push(bitmap::share(bitmap, budget)?);
            }
        }
        Ok(())
    }
}

/// A symbol height or width: an accumulated sum of decoded differences,
/// which must be positive and fit 32 bits.
fn positive(value: i64, message: &'static str) -> Result<u32> {
    u32::try_from(value)
        .ok()
        .filter(|&n| n != 0)
        .ok_or_else(|| Error::syntax(message))
}

fn select(
    symbols: &[Rc<Bitmap>],
    exported: usize,
    decoder: &mut Decoder<'_>,
    budget: &mut Budget,
) -> Result<Vec<Rc<Bitmap>>> {
    let mut exports = Integer::new();
    let mut selected = Vec::new();
    budget.reserve(&mut selected, exported)?;
    let mut index = 0;
    let mut export = false;
    while index < symbols.len() {
        budget.work(1)?;
        let length = usize::try_from(exports.required(decoder)?)
            .ok()
            .filter(|&n| n <= symbols.len() - index)
            .ok_or_else(|| Error::syntax("invalid JBIG2 symbol export run"))?;
        if export {
            if length > exported.saturating_sub(selected.len()) {
                return Err(Error::syntax("too many JBIG2 exported symbols"));
            }
            selected.extend(symbols[index..index + length].iter().cloned());
        }
        export = !export;
        index += length;
    }
    if selected.len() != exported {
        return Err(Error::syntax("JBIG2 exported symbol count mismatch"));
    }
    Ok(selected)
}
