//! Arithmetic dictionaries and retained contexts, T.88 (02/2000), 6.5/7.4.2.
pub(super) mod huffman;

use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    budget::Work,
    generic::{self, Template},
    integer::Integer,
    reader::Reader,
    refinement::{Model, Parameters},
    text,
};
use pagura_core::{Error, Limits, Position, Result};
use std::rc::Rc;

pub(super) struct Dictionary {
    pub(super) symbols: Vec<Rc<Bitmap>>,
    pub(super) bytes: usize,
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
    aggregation: Option<text::State>,
}

pub(super) fn decode(
    reader: &mut Reader<'_>,
    imported: &[Rc<Bitmap>],
    retained: Option<&Retained>,
    limits: &Limits,
    work: &mut Work,
) -> Result<Dictionary> {
    let config = Config::read(reader)?;
    let exported = usize::try_from(reader.u32()?)
        .map_err(|_| Error::limit("JBIG2 exported symbol count overflow"))?;
    let added = usize::try_from(reader.u32()?)
        .map_err(|_| Error::limit("JBIG2 new symbol count overflow"))?;
    let total = imported
        .len()
        .checked_add(added)
        .filter(|&n| n <= limits.value_nodes && n <= limits.objects)
        .ok_or_else(|| Error::limit("JBIG2 symbol count exceeds budget"))?;
    if exported > total {
        return Err(Error::syntax(
            "JBIG2 exported symbol count exceeds dictionary",
        ));
    }
    let overhead = total
        .checked_mul(2 * size_of::<Rc<Bitmap>>() + size_of::<Bitmap>() + 2 * size_of::<usize>())
        .and_then(|n| n.checked_add(config.generic.context_bytes() + 4 * size_of::<Integer>()))
        .filter(|&n| n <= limits.decoded_bytes)
        .ok_or_else(|| Error::limit("JBIG2 symbol tables exceed byte budget"))?;
    let mut bitmap_limits = *limits;
    bitmap_limits.decoded_bytes -= overhead;
    let mut procedure = Procedure::new(&config, retained, total, &bitmap_limits)?;
    let aggregation_bytes = procedure
        .aggregation
        .as_ref()
        .map_or(0, text::State::context_bytes);
    let base = match reader.position() {
        Position::Buffer(p) => p,
        _ => 0,
    };
    let mut decoder = Decoder::new(reader.rest(), base)?;
    let mut symbols = Vec::with_capacity(total);
    symbols.extend(imported.iter().cloned());
    let bytes = procedure.new_symbols(
        &mut decoder,
        &config,
        &mut symbols,
        total,
        &bitmap_limits,
        work,
    )?;
    let selected = select(&symbols, exported, &mut decoder, limits, work)?;
    let contexts = if config.retain {
        Some(Retained {
            settings: config.settings,
            generic: procedure.generic,
            refinement: procedure
                .aggregation
                .and_then(text::State::into_refinement_contexts),
        })
    } else {
        None
    };
    Ok(Dictionary {
        symbols: selected,
        bytes: bytes + overhead + aggregation_bytes,
        contexts,
    })
}

/// All bitmap coding parameters must match on context reuse (7.4.2.2 step 3).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Settings {
    modes: u16,
    adaptive: [u8; 12],
    length: usize,
}
impl Settings {
    pub(super) fn new(flags: u16, bytes: &[u8]) -> Result<Self> {
        let mut adaptive = [0; 12];
        adaptive
            .get_mut(..bytes.len())
            .ok_or_else(|| Error::syntax("invalid JBIG2 bitmap parameter size"))?
            .copy_from_slice(bytes);
        Ok(Self {
            modes: flags & 0x1c03,
            adaptive,
            length: bytes.len(),
        })
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
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let flags = reader.u16()?;
        let adaptive_start = reader.offset;
        if flags & 0xfc != 0 || flags & 2 == 0 && flags & 0x1000 != 0 {
            return Err(Error::syntax("unused JBIG2 symbol coding flags"));
        }
        if flags & 0xe000 != 0 {
            return Err(Error::syntax("reserved JBIG2 symbol dictionary flags"));
        }
        // The segment dispatcher routes Huffman dictionaries to `huffman`.
        debug_assert_eq!(flags & 1, 0, "Huffman symbol dictionary");
        let template = u8::try_from((flags >> 10) & 3)
            .map_err(|_| Error::syntax("invalid JBIG2 symbol template"))?;
        let generic = Template::read(reader, template)?;
        let refinement = if flags & 2 != 0 {
            Some(Parameters::read(reader, u8::from(flags & 0x1000 != 0))?)
        } else {
            None
        };
        Ok(Self {
            settings: Settings::new(flags, &reader.data[adaptive_start..reader.offset])?,
            generic,
            refinement,
            reuse: flags & 0x100 != 0,
            retain: flags & 0x200 != 0,
        })
    }
}

impl Procedure {
    fn new(
        config: &Config,
        retained: Option<&Retained>,
        total: usize,
        limits: &Limits,
    ) -> Result<Self> {
        let retained = if config.reuse {
            Some(retained.ok_or_else(|| Error::syntax("missing JBIG2 retained symbol contexts"))?)
        } else {
            None
        };
        let generic = if let Some(retained) = retained {
            if retained.generic.len() != config.generic.context_count() {
                return Err(Error::syntax(
                    "incompatible JBIG2 retained generic contexts",
                ));
            }
            retained.generic.clone()
        } else {
            vec![Context::default(); config.generic.context_count()]
        };
        let aggregation = if let Some(parameters) = config.refinement {
            let previous = retained.and_then(|r| r.refinement.as_deref());
            if config.reuse && previous.is_none() {
                return Err(Error::syntax("missing JBIG2 retained refinement contexts"));
            }
            let model = Model::new(parameters, previous, limits)?;
            Some(text::State::new(total, Some(model), limits)?)
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
        work: &mut Work,
    ) -> Result<usize> {
        let mut height = 0i64;
        let mut bytes = 0;
        let mut bitmap_limits = *limits;
        while symbols.len() < total {
            work.operations(1, limits)?;
            height = height
                .checked_add(self.heights.required(decoder)?)
                .filter(|&n| n > 0 && n <= i64::from(u32::MAX))
                .ok_or_else(|| Error::syntax("invalid JBIG2 symbol height"))?;
            let height =
                u32::try_from(height).map_err(|_| Error::syntax("invalid JBIG2 symbol height"))?;
            let mut width = 0i64;
            while let Some(delta) = self.widths.decode(decoder)? {
                if symbols.len() == total {
                    return Err(Error::syntax("too many JBIG2 new symbols"));
                }
                width = width
                    .checked_add(delta)
                    .filter(|&n| n > 0 && n <= i64::from(u32::MAX))
                    .ok_or_else(|| Error::syntax("invalid JBIG2 symbol width"))?;
                let width = u32::try_from(width)
                    .map_err(|_| Error::syntax("invalid JBIG2 symbol width"))?;
                super::bitmap::layout(width, height, &bitmap_limits)?;
                work.pixels(u64::from(width) * u64::from(height), limits)?;
                let bitmap = if let Some(aggregation) = &mut self.aggregation {
                    let instances = usize::try_from(self.instances.required(decoder)?)
                        .ok()
                        .filter(|&n| n != 0)
                        .ok_or_else(|| Error::syntax("invalid JBIG2 aggregate instance count"))?;
                    if instances == 1 {
                        aggregation.single(
                            decoder,
                            symbols,
                            (width, height),
                            &bitmap_limits,
                            work,
                        )?
                    } else {
                        aggregation.decode(
                            decoder,
                            symbols,
                            text::Placement {
                                size: (width, height),
                                instances,
                                config: text::Config::aggregation(),
                            },
                            &bitmap_limits,
                            work,
                        )?
                    }
                } else {
                    generic::decode_arithmetic(
                        width,
                        height,
                        &config.generic,
                        false,
                        decoder,
                        &mut self.generic,
                        &bitmap_limits,
                    )?
                };
                bytes += bitmap.data.len();
                bitmap_limits.decoded_bytes -= bitmap.data.len();
                symbols.push(Rc::new(bitmap));
            }
        }
        Ok(bytes)
    }
}

fn select(
    symbols: &[Rc<Bitmap>],
    exported: usize,
    decoder: &mut Decoder<'_>,
    limits: &Limits,
    work: &mut Work,
) -> Result<Vec<Rc<Bitmap>>> {
    let mut exports = Integer::new();
    let mut selected = Vec::with_capacity(exported);
    let mut index = 0;
    let mut export = false;
    while index < symbols.len() {
        work.operations(1, limits)?;
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
