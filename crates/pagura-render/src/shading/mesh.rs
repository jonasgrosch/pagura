//! Mesh stream decoding and spatial queries (8.7.4.5.5–8.7.4.5.8).
mod patches;
mod spatial;
use super::Functions;
use crate::icc::ColorContext;
use pagura_core::{Dictionary, Error, Limits, Result, Value};
use pagura_raster::Point;
use patches::Patch;
use spatial::{Bounds, Index};

#[derive(Debug, Clone, Copy)]
struct Vertex {
    point: Point,
    color: [f64; 32],
}
#[derive(Debug)]
enum Primitive {
    Triangle(Box<[Vertex; 3]>),
    Patch(Box<Patch>),
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
                .map(|p| match p {
                    Primitive::Triangle(_) => size_of::<[Vertex; 3]>(),
                    Primitive::Patch(_) => size_of::<Patch>(),
                })
                .sum::<usize>()
            + self.index.retained_bytes()
    }

    pub(super) fn parse(
        document: &ColorContext<'_>,
        d: &Dictionary,
        value: &Value,
        kind: i64,
        components: usize,
    ) -> Result<Self> {
        let id = value
            .as_reference()
            .ok_or(Error::syntax("mesh shading must be a stream"))?;
        let data = document.stream_data(document.object(id)?)?;
        let function = d.get(b"Function");
        let inputs = if function.is_some() { 1 } else { components };
        let mut decoder = Decoder::new(document, d, &data, kind, inputs)?;
        let mut primitives = Vec::new();
        let limits = document.limits();
        if kind == 4 {
            free_triangles(&mut decoder, &mut primitives, &limits)?;
        } else if kind == 5 {
            lattice(
                &mut decoder,
                &mut primitives,
                &limits,
                integer(document, d, b"VerticesPerRow")?,
            )?;
        } else {
            patches::decode(&mut decoder, &mut primitives, &limits, kind == 6)?;
        }
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
        let index = Index::new(&primitives, &limits)?;
        Ok(Self {
            primitives,
            index,
            functions,
            inputs,
            maximum: limits.raster_work,
        })
    }
    pub(super) fn sample(
        &self,
        point: Point,
        out: &mut [f64; 32],
        work: &mut usize,
    ) -> Result<bool> {
        let mut hit = None;
        self.index.query(
            point,
            &mut |index, work| {
                charge(work, self.maximum)?;
                if hit.is_some_and(|previous| previous > index) {
                    return Ok(());
                }
                let value = match &self.primitives[index] {
                    Primitive::Triangle(v) => triangle(v, point, self.inputs),
                    Primitive::Patch(p) => p.sample(point, self.inputs, work, self.maximum)?,
                };
                if let Some(value) = value {
                    *out = value;
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
fn triangle(v: &[Vertex; 3], p: Point, n: usize) -> Option<[f64; 32]> {
    let cross =
        |a: Point, b: Point, c: Point| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let area = cross(v[0].point, v[1].point, v[2].point);
    if area == 0.0 {
        return None;
    }
    let w1 = cross(v[0].point, p, v[2].point) / area;
    let w2 = cross(v[0].point, v[1].point, p) / area;
    let w0 = 1.0 - w1 - w2;
    if w0.min(w1).min(w2) < -1e-12 {
        return None;
    }
    let mut out = [0.0; 32];
    for (i, out) in out.iter_mut().enumerate().take(n) {
        *out = w0 * v[0].color[i] + w1 * v[1].color[i] + w2 * v[2].color[i];
    }
    Some(out)
}
fn push(out: &mut Vec<Primitive>, v: Primitive, limits: &Limits) -> Result<()> {
    let bytes = size_of::<Primitive>()
        + size_of::<Patch>()
        + 2 * size_of::<spatial::Node>()
        + size_of::<usize>();
    let maximum = limits.path_segments.min(limits.decoded_bytes / bytes);
    if out.len() >= maximum {
        return Err(Error::limit("mesh geometry memory budget exceeded"));
    }
    if out.len() == out.capacity() {
        let capacity = out.capacity().saturating_mul(2).max(8).min(maximum);
        out.try_reserve_exact(capacity - out.len())
            .map_err(|_| Error::limit("mesh allocation failed"))?;
    }
    out.push(v);
    Ok(())
}
fn free_triangles(d: &mut Decoder<'_>, out: &mut Vec<Primitive>, limits: &Limits) -> Result<()> {
    let mut previous = None;
    while d.more() {
        let flag = d.flag()?;
        let v = d.vertex()?;
        d.align();
        let vertices = match flag {
            0 => {
                d.flag()?;
                let b = d.vertex()?;
                d.align();
                d.flag()?;
                let c = d.vertex()?;
                d.align();
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
        push(out, Primitive::Triangle(Box::new(vertices)), limits)?;
    }
    Ok(())
}
fn lattice(
    d: &mut Decoder<'_>,
    out: &mut Vec<Primitive>,
    limits: &Limits,
    row: usize,
) -> Result<()> {
    if row < 2 {
        return Err(Error::syntax("VerticesPerRow must be at least two"));
    }
    if row
        .checked_mul(2 * size_of::<Vertex>())
        .is_none_or(|n| n > limits.decoded_bytes)
    {
        return Err(Error::limit("invalid or excessive mesh row width"));
    }
    let mut previous = Vec::new();
    let mut current = Vec::new();
    previous
        .try_reserve_exact(row)
        .map_err(|_| Error::limit("mesh row allocation failed"))?;
    current
        .try_reserve_exact(row)
        .map_err(|_| Error::limit("mesh row allocation failed"))?;
    while d.more() {
        current.push(d.vertex()?);
        d.align();
        if current.len() == row {
            if !previous.is_empty() {
                for i in 0..row - 1 {
                    push(
                        out,
                        Primitive::Triangle(Box::new([previous[i], previous[i + 1], current[i]])),
                        limits,
                    )?;
                    push(
                        out,
                        Primitive::Triangle(Box::new([
                            previous[i + 1],
                            current[i],
                            current[i + 1],
                        ])),
                        limits,
                    )?;
                }
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
    data: &'a [u8],
    bit: usize,
    coordinate: usize,
    component: usize,
    flag: usize,
    inputs: usize,
    decode: Vec<[f64; 2]>,
}
impl<'a> Decoder<'a> {
    fn new(
        document: &ColorContext<'_>,
        d: &Dictionary,
        data: &'a [u8],
        kind: i64,
        inputs: usize,
    ) -> Result<Self> {
        let coordinate = integer(document, d, b"BitsPerCoordinate")?;
        let component = integer(document, d, b"BitsPerComponent")?;
        let flag = if kind == 5 {
            0
        } else {
            integer(document, d, b"BitsPerFlag")?
        };
        if !matches!(coordinate, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32)
            || !matches!(component, 1 | 2 | 4 | 8 | 12 | 16)
            || !(if kind == 5 {
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
                    crate::numbers::number(document, low, crate::numbers::GEOMETRY)?,
                    crate::numbers::number(document, high, crate::numbers::GEOMETRY)?,
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            data,
            bit: 0,
            coordinate,
            component,
            flag,
            inputs,
            decode,
        })
    }
    fn more(&self) -> bool {
        self.bit / 8 < self.data.len()
    }
    fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }
    fn bits(&mut self, n: usize) -> Result<u32> {
        let end = self
            .bit
            .checked_add(n)
            .filter(|n| n.div_ceil(8) <= self.data.len())
            .ok_or(Error::syntax("truncated mesh record"))?;
        let mut value = 0;
        while self.bit < end {
            value = (value << 1) | u32::from((self.data[self.bit / 8] >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(value)
    }
    fn flag(&mut self) -> Result<u8> {
        u8::try_from(self.bits(self.flag)? & 3).map_err(|_| Error::syntax("invalid mesh edge flag"))
    }
    fn value(&mut self, bits: usize, index: usize) -> Result<f64> {
        let raw = self.bits(bits)?;
        let [lo, hi] = self.decode[index];
        let max = if bits == 32 {
            u32::MAX
        } else {
            (1u32 << bits) - 1
        };
        Ok(lo + (hi - lo) * f64::from(raw) / f64::from(max))
    }
    fn point(&mut self) -> Result<Point> {
        Ok(Point::new(
            self.value(self.coordinate, 0)?,
            self.value(self.coordinate, 1)?,
        ))
    }
    fn color(&mut self) -> Result<[f64; 32]> {
        let mut c = [0.0; 32];
        for (i, c) in c.iter_mut().enumerate().take(self.inputs) {
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
