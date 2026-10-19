//! Mesh stream decoding and spatial queries (8.7.4.5.5–8.7.4.5.8).
mod patches;
mod spatial;
use super::{Functions, ShadingType};
use crate::color::Components;
use crate::icc::ColorContext;
use pagura_core::bytes::{BitCursor, Cursor};
use pagura_core::syntax::Origin;
use pagura_core::{Dictionary, Error, Meter, Result, Value};
use pagura_raster::Point;
use patches::Patch;
use spatial::{Bounds, Index};

/// A decoded vertex; `color` holds the mesh's inputs.
#[derive(Debug, Clone, Copy)]
struct Vertex {
    point: Point,
    color: Components,
}
/// Corner points and their colours, `inputs` values per corner.
#[derive(Debug)]
struct Triangle {
    points: [Point; 3],
    colors: Box<[f64]>,
}
#[derive(Debug)]
enum Primitive {
    Triangle(Triangle),
    Patch(Box<Patch>),
}
impl Primitive {
    fn triangle(vertices: &[Vertex; 3], bytes: &mut Meter) -> Result<Self> {
        Ok(Self::Triangle(Triangle {
            points: vertices.map(|v| v.point),
            colors: colors(&vertices.map(|v| v.color), bytes)?,
        }))
    }
    /// Heap bytes beyond the primitive itself.
    fn owned_bytes(&self) -> usize {
        match self {
            Self::Triangle(t) => size_of_val(&*t.colors),
            Self::Patch(p) => size_of::<Patch>() + size_of_val(&*p.colors),
        }
    }
}
/// The corner colours of one primitive, flattened, charged to `bytes`.
fn colors(corners: &[Components], bytes: &mut Meter) -> Result<Box<[f64]>> {
    let mut values = Vec::new();
    let count = corners.iter().map(|c| c.len()).sum();
    bytes.reserve(&mut values, count)?;
    for corner in corners {
        values.extend_from_slice(corner);
    }
    Ok(values.into_boxed_slice())
}
/// Barycentric or bilinear interpolation of corner colours into `out`.
fn interpolate(colors: &[f64], weights: &[f64], out: &mut [f64]) {
    let inputs = out.len();
    out.fill(0.0);
    for (corner, weight) in colors.chunks_exact(inputs).zip(weights) {
        for (out, value) in out.iter_mut().zip(corner) {
            *out += weight * value;
        }
    }
}
#[derive(Debug)]
pub(super) struct Mesh {
    primitives: Vec<Primitive>,
    index: Index,
    pub(super) functions: Functions,
    inputs: usize,
    maximum: usize,
}
impl Mesh {
    pub(super) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.primitives.capacity() * size_of::<Primitive>()
            + self
                .primitives
                .iter()
                .map(Primitive::owned_bytes)
                .sum::<usize>()
            + self.index.retained_bytes()
    }

    /// Colour values per vertex: the shading's components, or the one
    /// parameter of its Function.
    pub(super) const fn inputs(&self) -> usize {
        self.inputs
    }

    pub(super) fn parse(
        document: &ColorContext<'_>,
        d: &Dictionary,
        value: &Value,
        kind: ShadingType,
        components: usize,
    ) -> Result<Self> {
        let id = value
            .as_reference()
            .ok_or(Error::syntax("mesh shading must be a stream"))?;
        let data = document.stream_data(document.object(id)?)?;
        let function = d.get(b"Function");
        let inputs = if function.is_some() { 1 } else { components };
        let cursor = Cursor::new(&data)
            .with_origin(Origin::Stream(id))
            .with_message("truncated mesh record");
        let mut decoder = Decoder::new(document, d, BitCursor::new(cursor), kind, inputs)?;
        let limits = document.limits();
        let mut out = Decoded {
            primitives: Vec::new(),
            bytes: Meter::new(limits.decoded_bytes, "mesh memory budget exceeded"),
            maximum: limits.path_segments,
        };
        if kind == ShadingType::FreeTriangles {
            free_triangles(&mut decoder, &mut out)?;
        } else if kind == ShadingType::LatticeTriangles {
            lattice(
                &mut decoder,
                &mut out,
                integer(document, d, b"VerticesPerRow")?,
            )?;
        } else {
            patches::decode(&mut decoder, &mut out, kind)?;
        }
        let Decoded {
            primitives,
            mut bytes,
            ..
        } = out;
        if primitives.is_empty() {
            return Err(Error::syntax("mesh shading has no complete geometry"));
        }
        let functions = if let Some(function) = function {
            let pair = decoder.decode[2];
            Functions::parse(
                document,
                function,
                &[[pair[0].min(pair[1]), pair[0].max(pair[1])]],
                components,
            )?
        } else {
            Functions::Identity
        };
        let index = Index::new(&primitives, &mut bytes)?;
        Ok(Self {
            primitives,
            index,
            functions,
            inputs,
            maximum: limits.raster_work,
        })
    }
    /// Writes the colour at `point` of the last primitive in stream order
    /// covering it into `out`, which holds [`Mesh::inputs`] values.
    pub(super) fn sample(&self, point: Point, out: &mut [f64], work: &mut usize) -> Result<bool> {
        let mut hit = None;
        self.index.query(
            point,
            &mut |index, work| {
                charge(work, self.maximum)?;
                if hit.is_some_and(|previous| previous > index) {
                    return Ok(());
                }
                let Some(primitive) = self.primitives.get(index) else {
                    return Ok(());
                };
                let found = match primitive {
                    Primitive::Triangle(t) => triangle(t, point, out),
                    Primitive::Patch(p) => p.sample(point, out, work, self.maximum)?,
                };
                if found {
                    hit = Some(index);
                }
                Ok(())
            },
            work,
            self.maximum,
        )?;
        Ok(hit.is_some())
    }
}
fn triangle(t: &Triangle, p: Point, out: &mut [f64]) -> bool {
    let cross =
        |a: Point, b: Point, c: Point| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let [a, b, c] = t.points;
    let area = cross(a, b, c);
    if area == 0.0 {
        return false;
    }
    let w1 = cross(a, p, c) / area;
    let w2 = cross(a, b, p) / area;
    let w0 = 1.0 - w1 - w2;
    if w0.min(w1).min(w2) < -1e-12 {
        return false;
    }
    interpolate(&t.colors, &[w0, w1, w2], out);
    true
}
/// Decoded primitives in stream order, their memory budget and count cap.
struct Decoded {
    primitives: Vec<Primitive>,
    bytes: Meter,
    maximum: usize,
}
impl Decoded {
    fn push(&mut self, primitive: Primitive) -> Result<()> {
        if self.primitives.len() >= self.maximum {
            return Err(Error::limit("mesh geometry budget exceeded"));
        }
        // The colours are already charged; charge the primitive and its box.
        self.bytes.charge(
            size_of::<Primitive>()
                + match &primitive {
                    Primitive::Triangle(_) => 0,
                    Primitive::Patch(_) => size_of::<Patch>(),
                },
        )?;
        if self.primitives.len() == self.primitives.capacity() {
            let capacity = self
                .primitives
                .capacity()
                .saturating_mul(2)
                .clamp(8, self.maximum);
            self.primitives
                .try_reserve_exact(capacity - self.primitives.len())
                .map_err(|_| Error::limit("mesh allocation failed"))?;
        }
        self.primitives.push(primitive);
        Ok(())
    }
}
fn free_triangles(d: &mut Decoder<'_>, out: &mut Decoded) -> Result<()> {
    let mut previous = None;
    while d.more() {
        let flag = d.flag()?;
        let v = d.vertex()?;
        d.align()?;
        let vertices = match flag {
            0 => {
                d.flag()?;
                let b = d.vertex()?;
                d.align()?;
                d.flag()?;
                let c = d.vertex()?;
                d.align()?;
                [v, b, c]
            }
            1 | 2 => {
                let p =
                    previous.ok_or(Error::syntax("mesh continuation has no preceding triangle"))?;
                let p: [Vertex; 3] = p;
                [p[usize::from(flag == 1)], p[2], v]
            }
            _ => return Err(Error::syntax("invalid triangle edge flag")),
        };
        previous = Some(vertices);
        let triangle = Primitive::triangle(&vertices, &mut out.bytes)?;
        out.push(triangle)?;
    }
    Ok(())
}
fn lattice(d: &mut Decoder<'_>, out: &mut Decoded, row: usize) -> Result<()> {
    if row < 2 {
        return Err(Error::syntax("VerticesPerRow must be at least two"));
    }
    // Two rows of vertices are held while their triangles are built.
    let mut rows = Meter::new(out.bytes.remaining(), "invalid or excessive mesh row width");
    let mut previous = Vec::new();
    let mut current = Vec::new();
    rows.reserve(&mut previous, row)?;
    rows.reserve(&mut current, row)?;
    while d.more() {
        current.push(d.vertex()?);
        d.align()?;
        if current.len() == row {
            for (above, below) in previous.windows(2).zip(current.windows(2)) {
                let ([a, b], [c, e]) = (above, below) else {
                    continue;
                };
                let first = Primitive::triangle(&[*a, *b, *c], &mut out.bytes)?;
                out.push(first)?;
                let second = Primitive::triangle(&[*b, *c, *e], &mut out.bytes)?;
                out.push(second)?;
            }
            std::mem::swap(&mut current, &mut previous);
            current.clear();
        }
    }
    if !current.is_empty() {
        return Err(Error::syntax("incomplete mesh row"));
    }
    Ok(())
}
struct Decoder<'a> {
    bits: BitCursor<'a>,
    coordinate: u32,
    component: u32,
    flag: u32,
    inputs: usize,
    decode: Vec<[f64; 2]>,
}
impl<'a> Decoder<'a> {
    fn new(
        document: &ColorContext<'_>,
        d: &Dictionary,
        bits: BitCursor<'a>,
        kind: ShadingType,
        inputs: usize,
    ) -> Result<Self> {
        let coordinate = integer(document, d, b"BitsPerCoordinate")?;
        let component = integer(document, d, b"BitsPerComponent")?;
        let flag = if kind == ShadingType::LatticeTriangles {
            0
        } else {
            integer(document, d, b"BitsPerFlag")?
        };
        if !matches!(coordinate, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32)
            || !matches!(component, 1 | 2 | 4 | 8 | 12 | 16)
            || !(if kind == ShadingType::LatticeTriangles {
                flag == 0
            } else {
                matches!(flag, 2 | 4 | 8)
            })
        {
            return Err(Error::syntax("invalid mesh bit width"));
        }
        let decode = document
            .resolve(
                d.get(b"Decode")
                    .ok_or(Error::syntax("mesh requires Decode"))?,
            )?
            .as_array()
            .filter(|v| v.len() == 4 + 2 * inputs)
            .ok_or(Error::syntax("invalid mesh Decode dimensions"))?;
        let decode = decode
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[low, high]| {
                Ok([
                    crate::numbers::number(document.document(), low, crate::numbers::GEOMETRY)?,
                    crate::numbers::number(document.document(), high, crate::numbers::GEOMETRY)?,
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        let width =
            |bits: usize| u32::try_from(bits).map_err(|_| Error::syntax("invalid mesh bit width"));
        Ok(Self {
            bits,
            coordinate: width(coordinate)?,
            component: width(component)?,
            flag: width(flag)?,
            inputs,
            decode,
        })
    }
    /// Whether another record starts; records begin byte-aligned.
    const fn more(&self) -> bool {
        self.bits.remaining_bits() > 0
    }
    fn align(&mut self) -> Result<()> {
        self.bits.align().map(|_| ())
    }
    fn flag(&mut self) -> Result<u8> {
        // Only the low two bits carry an edge flag (8.7.4.5.5).
        Ok(self.bits.bits(self.flag)?.to_le_bytes()[0] & 3)
    }
    fn value(&mut self, bits: u32, index: usize) -> Result<f64> {
        let raw = self.bits.bits(bits)?;
        let [lo, hi] = self
            .decode
            .get(index)
            .copied()
            .ok_or(Error::syntax("invalid mesh Decode dimensions"))?;
        let max = u32::MAX >> (32 - bits);
        Ok(lo + (hi - lo) * f64::from(raw) / f64::from(max))
    }
    fn point(&mut self) -> Result<Point> {
        Ok(Point::new(
            self.value(self.coordinate, 0)?,
            self.value(self.coordinate, 1)?,
        ))
    }
    fn color(&mut self) -> Result<Components> {
        let mut c = Components::zeros(self.inputs);
        for (i, c) in c.iter_mut().enumerate() {
            *c = self.value(self.component, 2 + i)?;
        }
        Ok(c)
    }
    fn vertex(&mut self) -> Result<Vertex> {
        Ok(Vertex {
            point: self.point()?,
            color: self.color()?,
        })
    }
}
fn integer(document: &ColorContext<'_>, d: &Dictionary, key: &[u8]) -> Result<usize> {
    d.get(key)
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_integer)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(Error::syntax("mesh integer parameter missing or invalid"))
}
fn charge(work: &mut usize, maximum: usize) -> Result<()> {
    *work = work
        .checked_add(1)
        .filter(|n| *n <= maximum)
        .ok_or(Error::limit("mesh evaluation work exceeded"))?;
    Ok(())
}
