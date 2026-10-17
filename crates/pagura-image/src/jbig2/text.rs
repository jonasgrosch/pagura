//! Arithmetic text and symbol-aggregation placement. T.88 primary-text
//! review is pending; independently encoded bitmaps are checked with PDFium.
use super::{
    Bitmap,
    arithmetic::{Context, Decoder},
    budget::Work,
    integer::{Integer, symbol_id},
    reader::Reader,
    refinement::{self, Model, Parameters},
    tags,
};
use pagura_core::{Error, Limits, Position, Result};
use std::rc::Rc;

pub(super) fn decode(
    reader: &mut Reader<'_>,
    size: (u32, u32),
    symbols: &[Rc<Bitmap>],
    limits: &Limits,
    work: &mut Work,
) -> Result<Bitmap> {
    let (config, parameters) = Config::read(reader)?;
    let instances = usize::try_from(reader.u32()?)
        .map_err(|_| Error::limit("JBIG2 text instance count overflow"))?;
    let model = parameters
        .map(|p| Model::new(p, None, limits))
        .transpose()?;
    let mut state = State::new(symbols.len(), model, limits)?;
    let base = match reader.position() {
        Position::Buffer(p) => p,
        _ => 0,
    };
    let mut decoder = Decoder::new(reader.rest(), base)?;
    state.decode(
        &mut decoder,
        symbols,
        Placement {
            size,
            instances,
            config,
        },
        limits,
        work,
    )
}

pub(super) struct State {
    identifiers: Vec<Context>,
    delta_t: Integer,
    first_s: Integer,
    delta_s: Integer,
    current_t: Integer,
    refinement: Option<Refinement>,
    bytes: usize,
}

struct Refinement {
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

impl State {
    pub(super) fn new(symbol_count: usize, model: Option<Model>, limits: &Limits) -> Result<Self> {
        let count = symbol_count
            .max(1)
            .checked_next_power_of_two()
            .ok_or_else(|| Error::limit("JBIG2 identifier context count overflow"))?;
        let overhead = count
            .checked_mul(size_of::<Context>())
            .and_then(|n| n.checked_add(size_of::<Self>()))
            .and_then(|n| n.checked_add(model.as_ref().map_or(0, Model::context_bytes)))
            .filter(|&n| n <= limits.decoded_bytes)
            .ok_or_else(|| Error::limit("JBIG2 text contexts exceed byte budget"))?;
        Ok(Self {
            bytes: overhead,
            identifiers: vec![Context::default(); count],
            delta_t: Integer::new(),
            first_s: Integer::new(),
            delta_s: Integer::new(),
            current_t: Integer::new(),
            refinement: model.map(|model| Refinement {
                model,
                indicator: Integer::new(),
                width: Integer::new(),
                height: Integer::new(),
                x: Integer::new(),
                y: Integer::new(),
            }),
        })
    }

    pub(super) fn context_bytes(&self) -> usize {
        self.bytes
    }

    pub(super) fn into_refinement_contexts(self) -> Option<Vec<Context>> {
        self.refinement.map(|r| r.model.contexts)
    }

    pub(super) fn decode(
        &mut self,
        decoder: &mut Decoder<'_>,
        symbols: &[Rc<Bitmap>],
        placement: Placement,
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Bitmap> {
        let Placement {
            size: (width, height),
            instances,
            config,
        } = placement;
        work.operations(instances, limits)?;
        if instances != 0 && symbols.is_empty() {
            return Err(Error::syntax("JBIG2 text region has no symbols"));
        }
        let mut bitmap_limits = *limits;
        bitmap_limits.decoded_bytes = limits
            .decoded_bytes
            .checked_sub(self.context_bytes())
            .ok_or_else(|| Error::limit("JBIG2 text contexts exceed byte budget"))?;
        let mut bitmap = Bitmap::new(width, height, config.default_black, &bitmap_limits)?;
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
                let id = symbol_id(decoder, &mut self.identifiers)?;
                let symbol = symbols
                    .get(id)
                    .ok_or_else(|| Error::syntax("JBIG2 symbol identifier out of range"))?;
                let mut glyph_limits = *limits;
                glyph_limits.decoded_bytes =
                    glyph_limits.decoded_bytes.saturating_sub(bitmap.data.len());
                let refined = self.refined(decoder, symbol, &glyph_limits, work)?;
                let glyph = refined.as_ref().unwrap_or(symbol);
                work.pixels(u64::from(glyph.width) * u64::from(glyph.height), limits)?;
                bitmap.combine_signed(glyph, s, t, config.operator);
                count += 1;
                if count == instances && !config.complete_last_strip {
                    break;
                }
                s = add(s, i64::from(glyph.width) - 1)?;
                let Some(delta) = self.delta_s.decode(decoder)? else {
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

    fn refined(
        &mut self,
        decoder: &mut Decoder<'_>,
        symbol: &Bitmap,
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Option<Bitmap>> {
        let overhead = self.context_bytes();
        let Some(refinement) = &mut self.refinement else {
            return Ok(None);
        };
        match refinement.indicator.required(decoder)? {
            0 => return Ok(None),
            1 => {}
            _ => return Err(Error::syntax("invalid JBIG2 text refinement indicator")),
        }
        let dw = refinement.width.required(decoder)?;
        let dh = refinement.height.required(decoder)?;
        let width = dimension(symbol.width, dw)?;
        let height = dimension(symbol.height, dh)?;
        let dx = (dw >> 1) + refinement.x.required(decoder)?;
        let dy = (dh >> 1) + refinement.y.required(decoder)?;
        let mut limits = *limits;
        limits.decoded_bytes = limits
            .decoded_bytes
            .saturating_sub(overhead - refinement.model.context_bytes());
        refinement
            .model
            .decode(
                decoder,
                symbol,
                refinement::Request {
                    width,
                    height,
                    dx,
                    dy,
                    prediction: false,
                },
                &limits,
                work,
            )
            .map(Some)
    }

    pub(super) fn single(
        &mut self,
        decoder: &mut Decoder<'_>,
        symbols: &[Rc<Bitmap>],
        size: (u32, u32),
        limits: &Limits,
        work: &mut Work,
    ) -> Result<Bitmap> {
        let overhead = self.context_bytes();
        let id = symbol_id(decoder, &mut self.identifiers)?;
        let symbol = symbols
            .get(id)
            .ok_or_else(|| Error::syntax("JBIG2 refinement symbol identifier out of range"))?;
        let refinement = self
            .refinement
            .as_mut()
            .ok_or_else(|| Error::syntax("missing JBIG2 symbol refinement contexts"))?;
        let dx = refinement.x.required(decoder)?;
        let dy = refinement.y.required(decoder)?;
        let mut limits = *limits;
        limits.decoded_bytes = limits
            .decoded_bytes
            .saturating_sub(overhead - refinement.model.context_bytes());
        refinement.model.decode(
            decoder,
            symbol,
            refinement::Request {
                width: size.0,
                height: size.1,
                dx,
                dy,
                prediction: false,
            },
            &limits,
            work,
        )
    }
}

fn dimension(base: u32, delta: i64) -> Result<u32> {
    u32::try_from(i64::from(base) + delta)
        .ok()
        .filter(|&n| n != 0)
        .ok_or_else(|| Error::syntax("invalid JBIG2 refined symbol dimension"))
}

pub(super) struct Config {
    strips: i64,
    operator: u8,
    offset: i64,
    default_black: bool,
    complete_last_strip: bool,
}

impl Config {
    pub(super) fn aggregation() -> Self {
        Self {
            strips: 1,
            operator: 0,
            offset: 0,
            default_black: false,
            complete_last_strip: true,
        }
    }
    fn read(reader: &mut Reader<'_>) -> Result<(Self, Option<Parameters>)> {
        let flags = reader.u16()?;
        if flags & 1 != 0 {
            return Err(Error::unsupported(
                tags::TEXT_REGION,
                "JBIG2 Huffman text regions are not implemented",
            ));
        }
        if flags & 0x70 != 0x10 {
            return Err(Error::unsupported(
                tags::TEXT_LAYOUT,
                "JBIG2 text reference corner or transposition is not implemented",
            ));
        }
        let parameters = if flags & 2 != 0 {
            Some(Parameters::read(reader, u8::from(flags & 0x8000 != 0))?)
        } else {
            None
        };
        let strips = 1i64 << ((flags >> 2) & 3);
        let operator = u8::try_from((flags >> 7) & 3)
            .map_err(|_| Error::syntax("invalid JBIG2 text operator"))?;
        let raw = i64::from((flags >> 10) & 31);
        Ok((
            Self {
                strips,
                operator,
                offset: if raw >= 16 { raw - 32 } else { raw },
                default_black: flags & 512 != 0,
                complete_last_strip: false,
            },
            parameters,
        ))
    }
}

fn add(left: i64, right: i64) -> Result<i64> {
    left.checked_add(right)
        .ok_or_else(|| Error::limit("JBIG2 text coordinate overflow"))
}
