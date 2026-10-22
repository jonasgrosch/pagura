//! Mesh shading streams (8.7.4.5.5–8.7.4.5.8): free-form and lattice
//! triangles, Coons and tensor-product patches.

use super::super::numbers::{GEOMETRY, number};
use super::functions;
use crate::model::{MeshVertex, Patch, ShadingFunction, ShadingKind};
use pagura_color::{Components, MAX_COMPONENTS};
use pagura_core::{Dictionary, Error, Limits, Result, Value};
use pagura_document::Document;
use pagura_raster::Point;

/// Control points in stream order (8.7.4.5.8) to storage, where
/// `points[4 * i + j]` holds `p_ij`: the stream lists `p00 p01 p02 p03 p13
/// p23 p33 p32 p31 p30 p20 p10 p11 p12 p22 p21`.
const ORDER: [usize; 16] = [0, 1, 2, 3, 7, 11, 15, 14, 13, 12, 8, 4, 5, 6, 10, 9];

/// Reads a mesh shading of type `kind` with `components` colour components.
pub(super) fn read(
    document: &Document,
    d: &Dictionary,
    value: &Value,
    kind: i64,
    components: usize,
) -> Result<(ShadingKind, Option<ShadingFunction>)> {
    let id = value
        .as_reference()
        .ok_or(Error::syntax("mesh shading must be a stream"))?;
    let data = document.stream_data(document.object(id)?)?;
    let function = d.get(b"Function");
    let inputs = if function.is_some() { 1 } else { components };
    let mut decoder = Decoder::new(document, d, &data, kind, inputs)?;
    let limits = document.limits();
    let mut budget = Budget::new(&limits);
    let geometry = match kind {
        4 => ShadingKind::Triangles(free_triangles(&mut decoder, &mut budget)?.into()),
        5 => {
            let row = integer(document, d, b"VerticesPerRow")?;
            ShadingKind::Triangles(lattice(&mut decoder, &mut budget, &limits, row)?.into())
        }
        _ => ShadingKind::Patches(patches(&mut decoder, &mut budget, kind == 6)?.into()),
    };
    if budget.count == 0 {
        return Err(Error::syntax("mesh shading has no complete geometry"));
    }
    let function = function
        .map(|function| {
            let [low, high] = decoder.decode.get(2).copied().unwrap_or([0.0, 1.0]);
            functions(
                document,
                function,
                &[[low.min(high), low.max(high)]],
                components,
            )
        })
        .transpose()?;
    Ok((geometry, function))
}

/// The geometry budget: primitives are bounded by `path_segments` and by
/// `decoded_bytes` at a conservative size each.
struct Budget {
    count: usize,
    maximum: usize,
}

impl Budget {
    fn new(limits: &Limits) -> Self {
        let bytes = size_of::<Patch>() + 4 * size_of::<usize>() + 64;
        Self {
            count: 0,
            maximum: limits.path_segments.min(limits.decoded_bytes / bytes),
        }
    }

    fn push<T>(&mut self, out: &mut Vec<T>, value: T) -> Result<()> {
        if self.count >= self.maximum {
            return Err(Error::limit("mesh geometry memory budget exceeded"));
        }
        if out.len() == out.capacity() {
            let capacity = out.capacity().saturating_mul(2).max(8).min(self.maximum);
            out.try_reserve_exact(capacity.saturating_sub(out.len()).max(1))
                .map_err(|_| Error::limit("mesh allocation failed"))?;
        }
        out.push(value);
        self.count += 1;
        Ok(())
    }
}

#[derive(Clone)]
struct Vertex {
    point: Point,
    color: [f64; MAX_COMPONENTS],
}

impl Vertex {
    fn mesh(&self, inputs: usize) -> MeshVertex {
        MeshVertex {
            point: self.point,
            color: Components::from_slice(self.color.get(..inputs).unwrap_or_default()),
        }
    }
}

fn free_triangles(d: &mut Decoder<'_>, budget: &mut Budget) -> Result<Vec<[MeshVertex; 3]>> {
    let mut out = Vec::new();
    let mut previous: Option<[Vertex; 3]> = None;
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
                let [a, b, c] = previous
                    .clone()
                    .ok_or(Error::syntax("mesh continuation has no preceding triangle"))?;
                if flag == 1 { [b, c, v] } else { [a, c, v] }
            }
            _ => return Err(Error::syntax("invalid triangle edge flag")),
        };
        budget.push(&mut out, vertices.clone().map(|v| v.mesh(d.inputs)))?;
        previous = Some(vertices);
    }
    Ok(out)
}

fn lattice(
    d: &mut Decoder<'_>,
    budget: &mut Budget,
    limits: &Limits,
    row: usize,
) -> Result<Vec<[MeshVertex; 3]>> {
    if row < 2 {
        return Err(Error::syntax("VerticesPerRow must be at least two"));
    }
    if row
        .checked_mul(2 * size_of::<Vertex>())
        .is_none_or(|n| n > limits.decoded_bytes)
    {
        return Err(Error::limit("invalid or excessive mesh row width"));
    }
    let mut out = Vec::new();
    let mut previous: Vec<MeshVertex> = Vec::new();
    let mut current: Vec<MeshVertex> = Vec::new();
    previous
        .try_reserve_exact(row)
        .map_err(|_| Error::limit("mesh row allocation failed"))?;
    current
        .try_reserve_exact(row)
        .map_err(|_| Error::limit("mesh row allocation failed"))?;
    while d.more() {
        current.push(d.vertex()?.mesh(d.inputs));
        d.align();
        if current.len() == row {
            if !previous.is_empty() {
                for (above, below) in previous.windows(2).zip(current.windows(2)) {
                    let [a, b] = [&above[0], &above[1]];
                    let [c, e] = [&below[0], &below[1]];
                    budget.push(&mut out, [a.clone(), b.clone(), c.clone()])?;
                    budget.push(&mut out, [b.clone(), c.clone(), e.clone()])?;
                }
            }
            std::mem::swap(&mut current, &mut previous);
            current.clear();
        }
    }
    if !current.is_empty() {
        return Err(Error::syntax("incomplete mesh row"));
    }
    Ok(out)
}

fn patches(d: &mut Decoder<'_>, budget: &mut Budget, coons: bool) -> Result<Vec<Patch>> {
    let mut out = Vec::new();
    let mut previous: Option<([Point; 16], [[f64; MAX_COMPONENTS]; 4])> = None;
    while d.more() {
        let flag = usize::from(d.flag()?);
        let mut raw = [Point::default(); 16];
        let mut colors = [[0.0; MAX_COMPONENTS]; 4];
        if flag != 0 {
            let (points, old) =
                previous.ok_or(Error::syntax("patch continuation has no preceding patch"))?;
            for (i, p) in raw.iter_mut().enumerate().take(4) {
                *p = points[(flag * 3 + i) % 12];
            }
            colors[0] = old[flag];
            colors[1] = old[(flag + 1) % 4];
        }
        for p in raw
            .iter_mut()
            .take(if coons { 12 } else { 16 })
            .skip(if flag == 0 { 0 } else { 4 })
        {
            *p = d.point()?;
        }
        for c in colors.iter_mut().skip(if flag == 0 { 0 } else { 2 }) {
            *c = d.color()?;
        }
        d.align();
        previous = Some((raw, colors));
        let mut points = [Point::default(); 16];
        for (&index, &point) in ORDER.iter().zip(&raw) {
            points[index] = point;
        }
        if coons {
            coons_interior(&mut points);
        }
        let inputs = d.inputs;
        budget.push(
            &mut out,
            Patch {
                points,
                colors: colors.map(|c| Components::from_slice(c.get(..inputs).unwrap_or_default())),
            },
        )?;
    }
    Ok(out)
}

/// The four interior control points a Coons patch implies (8.7.4.5.7):
/// `p_ij = S_C(u, v) + S_D(u, v) − S_B(u, v)` at `u = i/3`, `v = j/3`,
/// which is Table 129's tensor form of the Coons surface.
fn coons_interior(points: &mut [Point; 16]) {
    let mix = |a: Point, b: Point, t: f64| Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
    let p = |points: &[Point; 16], i: usize, j: usize| points[4 * i + j];
    for (i, j) in [(1_u8, 1_u8), (1, 2), (2, 1), (2, 2)] {
        let u = f64::from(i) / 3.0;
        let v = f64::from(j) / 3.0;
        let (i, j) = (usize::from(i), usize::from(j));
        let sc = mix(p(points, i, 0), p(points, i, 3), v);
        let sd = mix(p(points, 0, j), p(points, 3, j), u);
        let sb = mix(
            mix(p(points, 0, 0), p(points, 3, 0), u),
            mix(p(points, 0, 3), p(points, 3, 3), u),
            v,
        );
        points[4 * i + j] = Point::new(sc.x + sd.x - sb.x, sc.y + sd.y - sb.y);
    }
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
        document: &Document,
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
                    number(document, low, GEOMETRY)?,
                    number(document, high, GEOMETRY)?,
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
        let mut value = 0_u32;
        while self.bit < end {
            let byte = self.data.get(self.bit / 8).copied().unwrap_or(0);
            value = (value << 1) | u32::from((byte >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Ok(value)
    }

    fn flag(&mut self) -> Result<u8> {
        u8::try_from(self.bits(self.flag)? & 3).map_err(|_| Error::syntax("invalid mesh edge flag"))
    }

    fn value(&mut self, bits: usize, index: usize) -> Result<f64> {
        let raw = self.bits(bits)?;
        let [lo, hi] = *self
            .decode
            .get(index)
            .ok_or(Error::syntax("invalid mesh Decode dimensions"))?;
        let max = if bits >= 32 {
            u32::MAX
        } else {
            (1_u32 << bits) - 1
        };
        Ok(lo + (hi - lo) * f64::from(raw) / f64::from(max))
    }

    fn point(&mut self) -> Result<Point> {
        Ok(Point::new(
            self.value(self.coordinate, 0)?,
            self.value(self.coordinate, 1)?,
        ))
    }

    fn color(&mut self) -> Result<[f64; MAX_COMPONENTS]> {
        let mut c = [0.0; MAX_COMPONENTS];
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

fn integer(document: &Document, d: &Dictionary, key: &[u8]) -> Result<usize> {
    d.get(key)
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_integer)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(Error::syntax("mesh integer parameter missing or invalid"))
}

/// The bytes a mesh retains, for the scene's accounting.
pub(crate) fn retained_bytes(kind: &ShadingKind) -> usize {
    match kind {
        ShadingKind::Triangles(triangles) => {
            triangles.len().saturating_mul(size_of::<[MeshVertex; 3]>())
        }
        ShadingKind::Patches(patches) => patches.len().saturating_mul(size_of::<Patch>()),
        _ => 0,
    }
}
