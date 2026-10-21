//! Test-only reading of the shadings in generated fixtures into scene
//! values, following the previous parser (`pagura-render` `shading.rs`,
//! `shading/mesh.rs`, `color.rs`, `functions.rs`). The mesh decoding also
//! keeps the previous primitive layout for the oracle.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "fixture values are small"
)]

use pagura_color::cie::Calibrated;
use pagura_color::{
    CalculatorFunction, ColorSpace, Colorant, Components, DeviceN, ExponentialFunction, Function,
    IccSpace, Indexed, Profile, SampledFunction, Separation, StitchingFunction,
};
use pagura_core::{Dictionary, Limits, Name, Rect, Value};
use pagura_document::Document;
use pagura_interpret::{MeshVertex, Patch, ShadingFunction, ShadingKind};
use pagura_raster::{Point, Transform};
use std::sync::Arc;

/// A shading read from a fixture.
#[derive(Debug)]
pub(super) struct Parsed {
    pub(super) name: String,
    pub(super) space: Arc<ColorSpace>,
    pub(super) function: Option<ShadingFunction>,
    pub(super) kind: ShadingKind,
    pub(super) bbox: Option<Rect>,
    pub(super) antialias: bool,
    pub(super) background: Option<Components>,
    /// The previous decoder's primitives, for meshes.
    pub(super) old_mesh: Vec<OldPrimitive>,
}

/// A mesh primitive in the previous layout: points `p_ij` at `4·j + i`.
#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "the previous layout, kept as it was"
)]
pub(super) enum OldPrimitive {
    Triangle([(Point, [f64; 32]); 3]),
    Patch {
        points: [Point; 16],
        colors: [[f64; 32]; 4],
    },
}

struct Reader<'a> {
    document: &'a Document,
    resources: &'a Dictionary,
}

/// Every shading of a fixture's page resources (`Shading` entries and
/// shading patterns), with its resource name.
pub(super) fn shadings(document: &Document) -> Vec<Parsed> {
    let mut result = Vec::new();
    let Ok(pages) = document.pages() else {
        return result;
    };
    for page in pages {
        let reader = Reader {
            document,
            resources: page.resources(),
        };
        reader.collect(page.resources(), &mut result, 0);
    }
    result
}

impl Reader<'_> {
    fn collect(&self, resources: &Dictionary, out: &mut Vec<Parsed>, depth: usize) {
        if depth > 4 {
            return;
        }
        let entries = |key: &[u8]| -> Vec<(Name, Value)> {
            resources
                .get(key)
                .and_then(|v| self.document.resolve_dictionary(v).ok())
                .map(|d| d.iter().map(|(n, v)| (n.clone(), v.clone())).collect())
                .unwrap_or_default()
        };
        let reader = Reader {
            document: self.document,
            resources,
        };
        for (name, value) in entries(b"Shading") {
            if let Some(parsed) = reader.shading(&value, &format!("{name:?}")) {
                out.push(parsed);
            }
        }
        for (name, value) in entries(b"Pattern") {
            let Ok(d) = self.document.resolve_dictionary(&value) else {
                continue;
            };
            if d.get(b"PatternType").and_then(Value::as_integer) == Some(2)
                && let Some(shading) = d.get(b"Shading")
                && let Some(parsed) = reader.shading(shading, &format!("pattern {name:?}"))
            {
                out.push(parsed);
            }
        }
        for (_, value) in entries(b"XObject") {
            if let Some(id) = value.as_reference()
                && let Ok(object) = self.document.object(id)
                && let Some(d) = object.dictionary()
                && let Some(r) = d.get(b"Resources")
                && let Ok(r) = self.document.resolve_dictionary(r)
            {
                self.collect(r, out, depth + 1);
            }
        }
    }

    fn resolve<'v>(&'v self, value: &'v Value) -> &'v Value {
        self.document.resolve(value).unwrap_or(value)
    }

    fn number(&self, value: &Value) -> Option<f64> {
        self.resolve(value).as_number()
    }

    fn numbers(&self, value: &Value) -> Option<Vec<f64>> {
        self.resolve(value)
            .as_array()?
            .iter()
            .map(|v| self.number(v))
            .collect()
    }

    fn pairs(&self, value: &Value) -> Option<Vec<[f64; 2]>> {
        let numbers = self.numbers(value)?;
        let (pairs, rest) = numbers.as_chunks::<2>();
        rest.is_empty().then(|| pairs.to_vec())
    }

    fn dictionary_of<'v>(&'v self, value: &'v Value) -> Option<&'v Dictionary> {
        self.document.resolve_dictionary(value).ok()
    }

    fn stream(&self, value: &Value) -> Option<Vec<u8>> {
        let object = self.document.object(value.as_reference()?).ok()?;
        Some(self.document.stream_data(object).ok()?.into_owned())
    }

    fn shading(&self, value: &Value, name: &str) -> Option<Parsed> {
        let d = self.dictionary_of(value)?;
        let kind = d
            .get(b"ShadingType")
            .and_then(|v| self.resolve(v).as_integer())?;
        let space = Arc::new(self.space(d.get(b"ColorSpace")?, 0)?);
        let components = space.components();
        let bbox = match d.get(b"BBox") {
            Some(v) => {
                let n = self.numbers(v)?;
                Some(Rect::from_corners(
                    *n.first()?,
                    *n.get(1)?,
                    *n.get(2)?,
                    *n.get(3)?,
                )?)
            }
            None => None,
        };
        let antialias = d
            .get(b"AntiAlias")
            .and_then(|v| self.resolve(v).as_bool())
            .unwrap_or(false);
        let background = match d.get(b"Background") {
            Some(v) => {
                let n = self.numbers(v)?;
                (n.len() == components).then(|| Components::from_slice(&n))
            }
            None => None,
        };
        let mut old_mesh = Vec::new();
        let (kind, function) = if kind >= 4 {
            let inputs = if d.contains_key(b"Function") {
                1
            } else {
                components
            };
            let decoder = self.mesh_decoder(d, value, kind, inputs)?;
            old_mesh = decoder.0;
            let function = match d.get(b"Function") {
                Some(f) => Some(self.shading_function(f, 1, components)?),
                None => None,
            };
            (decoder.1, function)
        } else {
            let geometry = self.geometry(d, kind)?;
            let inputs = if kind == 1 { 2 } else { 1 };
            (
                geometry,
                Some(self.shading_function(d.get(b"Function")?, inputs, components)?),
            )
        };
        Some(Parsed {
            name: name.to_owned(),
            space,
            function,
            kind,
            bbox,
            antialias,
            background,
            old_mesh,
        })
    }

    fn geometry(&self, d: &Dictionary, kind: i64) -> Option<ShadingKind> {
        if kind == 1 {
            let domain = d
                .get(b"Domain")
                .map_or(Some(vec![0.0, 1.0, 0.0, 1.0]), |v| self.numbers(v))?;
            let m = d
                .get(b"Matrix")
                .map_or(Some(vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), |v| {
                    self.numbers(v)
                })?;
            return Some(ShadingKind::Function {
                domain: domain.try_into().ok()?,
                matrix: Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]),
            });
        }
        let domain = d
            .get(b"Domain")
            .map_or(Some(vec![0.0, 1.0]), |v| self.numbers(v))?;
        let domain: [f64; 2] = domain.try_into().ok()?;
        let extend = match d.get(b"Extend") {
            Some(v) => {
                let a = self.resolve(v).as_array()?;
                [
                    self.resolve(a.first()?).as_bool()?,
                    self.resolve(a.get(1)?).as_bool()?,
                ]
            }
            None => [false; 2],
        };
        let c = self.numbers(d.get(b"Coords")?)?;
        if kind == 2 {
            Some(ShadingKind::Axial {
                start: Point::new(c[0], c[1]),
                end: Point::new(c[2], c[3]),
                domain,
                extend,
            })
        } else {
            if c[2] < 0.0 || c[5] < 0.0 {
                return None;
            }
            Some(ShadingKind::Radial {
                start: Point::new(c[0], c[1]),
                start_radius: c[2],
                end: Point::new(c[3], c[4]),
                end_radius: c[5],
                domain,
                extend,
            })
        }
    }

    fn shading_function(
        &self,
        value: &Value,
        inputs: usize,
        outputs: usize,
    ) -> Option<ShadingFunction> {
        if let Some(values) = self.resolve(value).as_array() {
            let functions: Option<Vec<Arc<Function>>> = values
                .iter()
                .map(|v| self.function(v, inputs, 1, 0))
                .collect();
            return Some(ShadingFunction::PerComponent(functions?.into()));
        }
        Some(ShadingFunction::Single(
            self.function(value, inputs, outputs, 0)?,
        ))
    }

    fn function(
        &self,
        value: &Value,
        inputs: usize,
        outputs: usize,
        depth: usize,
    ) -> Option<Arc<Function>> {
        if depth > 8 {
            return None;
        }
        let d = self.dictionary_of(value)?;
        let domain = self.pairs(d.get(b"Domain")?)?;
        if domain.len() != inputs {
            return None;
        }
        let range = match d.get(b"Range") {
            Some(r) => Some(self.pairs(r)?),
            None => None,
        };
        let kind = d
            .get(b"FunctionType")
            .and_then(|v| self.resolve(v).as_integer())?;
        let limits = Limits::default();
        let function = match kind {
            0 => {
                let size = self
                    .numbers(d.get(b"Size")?)?
                    .iter()
                    .map(|&v| v as u32)
                    .collect();
                Function::sampled(
                    SampledFunction {
                        domain,
                        range: range?,
                        size,
                        bits_per_sample: d.get(b"BitsPerSample").and_then(|v| self.number(v))?
                            as u8,
                        order: d.get(b"Order").and_then(|v| self.number(v)).unwrap_or(1.0) as u8,
                        encode: d.get(b"Encode").and_then(|v| self.pairs(v)),
                        decode: d.get(b"Decode").and_then(|v| self.pairs(v)),
                        samples: self.stream(value)?.into(),
                    },
                    &limits,
                )
            }
            2 => Function::exponential(ExponentialFunction {
                domain: *domain.first()?,
                range,
                c0: d.get(b"C0").map_or(Some(vec![0.0]), |v| self.numbers(v))?,
                c1: d.get(b"C1").map_or(Some(vec![1.0]), |v| self.numbers(v))?,
                exponent: self.number(d.get(b"N")?)?,
            }),
            3 => {
                let functions: Option<Vec<_>> = self
                    .resolve(d.get(b"Functions")?)
                    .as_array()?
                    .iter()
                    .map(|v| self.function(v, 1, outputs, depth + 1))
                    .collect();
                Function::stitching(StitchingFunction {
                    domain: *domain.first()?,
                    range,
                    functions: functions?,
                    bounds: self.numbers(d.get(b"Bounds")?)?,
                    encode: self.pairs(d.get(b"Encode")?)?,
                })
            }
            4 => Function::calculator(
                CalculatorFunction {
                    domain,
                    range: range?,
                    program: self.stream(value)?.into(),
                },
                &limits,
            ),
            _ => return None,
        };
        let function = function.ok()?;
        (function.outputs() == outputs).then(|| Arc::new(function))
    }

    fn space(&self, value: &Value, depth: usize) -> Option<ColorSpace> {
        if depth > 6 {
            return None;
        }
        let value = self.resolve(value);
        if let Some(name) = value.as_name() {
            return match name.as_bytes() {
                b"DeviceGray" | b"G" => Some(ColorSpace::DeviceGray),
                b"DeviceRGB" | b"RGB" => Some(ColorSpace::DeviceRgb),
                b"DeviceCMYK" | b"CMYK" => Some(ColorSpace::DeviceCmyk),
                b"Pattern" => None,
                other => {
                    let spaces = self.dictionary_of(self.resources.get(b"ColorSpace")?)?;
                    self.space(spaces.get(other)?, depth + 1)
                }
            };
        }
        let array = value.as_array()?;
        let family = array.first()?.as_name()?.as_bytes();
        let arg = |i: usize| array.get(i);
        Some(match family {
            b"DeviceGray" => ColorSpace::DeviceGray,
            b"DeviceRGB" => ColorSpace::DeviceRgb,
            b"DeviceCMYK" => ColorSpace::DeviceCmyk,
            b"CalGray" | b"CalRGB" | b"Lab" => {
                ColorSpace::Calibrated(Arc::new(self.calibrated(family, arg(1)?)?))
            }
            b"ICCBased" => {
                let id = arg(1)?.as_reference()?;
                let object = self.document.object(id).ok()?;
                let d = object.dictionary()?;
                let n = d.get(b"N").and_then(|v| self.resolve(v).as_integer())? as usize;
                let mut range = [[0.0, 1.0]; 4];
                if let Some(r) = d.get(b"Range") {
                    for (out, pair) in range.iter_mut().zip(self.pairs(r)?) {
                        *out = pair;
                    }
                }
                let data = self.document.stream_data(object).ok()?;
                let profile = Profile::parse(&data, n, &Limits::default()).ok()?;
                let device = match n {
                    1 => ColorSpace::DeviceGray,
                    3 => ColorSpace::DeviceRgb,
                    _ => ColorSpace::DeviceCmyk,
                };
                let alternate = match (&profile, d.get(b"Alternate")) {
                    (None, Some(a)) => self.space(a, depth + 1)?,
                    _ => device,
                };
                ColorSpace::Icc(Arc::new(IccSpace {
                    profile: profile.map(Arc::new),
                    alternate: Arc::new(alternate),
                    components: n as u8,
                    range,
                }))
            }
            b"Indexed" => {
                let base = self.space(arg(1)?, depth + 1)?;
                let high = self.number(arg(2)?)? as u8;
                let lookup = match self.resolve(arg(3)?) {
                    Value::String(bytes) => bytes.clone(),
                    _ => self.stream(arg(3)?)?,
                };
                ColorSpace::Indexed(Arc::new(Indexed {
                    base: Arc::new(base),
                    high,
                    lookup: lookup.into(),
                }))
            }
            b"Separation" => {
                let name = self.resolve(arg(1)?).as_name()?.clone();
                let alternate = self.space(arg(2)?, depth + 1)?;
                let tint = self.function(arg(3)?, 1, alternate.components(), 0)?;
                ColorSpace::Separation(Arc::new(Separation {
                    colorant: colorant(name),
                    alternate: Arc::new(alternate),
                    tint,
                }))
            }
            b"DeviceN" => {
                let names = self.resolve(arg(1)?).as_array()?;
                let colorants: Option<Vec<Colorant>> = names
                    .iter()
                    .map(|n| self.resolve(n).as_name().cloned().map(colorant))
                    .collect();
                let colorants = colorants?;
                let alternate = self.space(arg(2)?, depth + 1)?;
                let tint = self.function(arg(3)?, colorants.len(), alternate.components(), 0)?;
                ColorSpace::DeviceN(Arc::new(DeviceN {
                    colorants,
                    alternate: Arc::new(alternate),
                    tint,
                }))
            }
            _ => return None,
        })
    }

    fn calibrated(&self, family: &[u8], value: &Value) -> Option<Calibrated> {
        let d = self.dictionary_of(value)?;
        let get = |key: &[u8], default: Option<Vec<f64>>| -> Option<Vec<f64>> {
            d.get(key).map_or(default, |v| self.numbers(v))
        };
        let white: [f64; 3] = get(b"WhitePoint", None)?.try_into().ok()?;
        let black: [f64; 3] = get(b"BlackPoint", Some(vec![0.0; 3]))?.try_into().ok()?;
        match family {
            b"CalGray" => Calibrated::gray(
                white,
                black,
                d.get(b"Gamma").and_then(|v| self.number(v)).unwrap_or(1.0),
            )
            .ok(),
            b"CalRGB" => {
                let gamma: [f64; 3] = get(b"Gamma", Some(vec![1.0; 3]))?.try_into().ok()?;
                let m = get(
                    b"Matrix",
                    Some(vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
                )?;
                let matrix = [[m[0], m[3], m[6]], [m[1], m[4], m[7]], [m[2], m[5], m[8]]];
                Calibrated::rgb(white, black, gamma, matrix).ok()
            }
            _ => {
                let range: [f64; 4] = get(b"Range", Some(vec![-100.0, 100.0, -100.0, 100.0]))?
                    .try_into()
                    .ok()?;
                Calibrated::lab(white, black, range).ok()
            }
        }
    }

    /// The previous mesh decoder: old-layout primitives and the scene kind.
    fn mesh_decoder(
        &self,
        d: &Dictionary,
        value: &Value,
        kind: i64,
        inputs: usize,
    ) -> Option<(Vec<OldPrimitive>, ShadingKind)> {
        let data = self.stream(value)?;
        let integer = |key: &[u8]| {
            d.get(key)
                .and_then(|v| self.resolve(v).as_integer())
                .map(|v| v as usize)
        };
        let decode = self.pairs(d.get(b"Decode")?)?;
        if decode.len() != 2 + inputs {
            return None;
        }
        let mut decoder = Decoder {
            data: &data,
            bit: 0,
            coordinate: integer(b"BitsPerCoordinate")?,
            component: integer(b"BitsPerComponent")?,
            flag: if kind == 5 {
                0
            } else {
                integer(b"BitsPerFlag")?
            },
            inputs,
            decode,
        };
        if !matches!(decoder.coordinate, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32)
            || !matches!(decoder.component, 1 | 2 | 4 | 8 | 12 | 16)
            || !(if kind == 5 {
                decoder.flag == 0
            } else {
                matches!(decoder.flag, 2 | 4 | 8)
            })
        {
            return None;
        }
        let primitives = match kind {
            4 => decoder.free_triangles()?,
            5 => decoder.lattice(integer(b"VerticesPerRow")?)?,
            _ => decoder.patches(kind == 6)?,
        };
        if primitives.is_empty() {
            return None;
        }
        let vertex = |(point, color): &(Point, [f64; 32])| MeshVertex {
            point: *point,
            color: Components::from_slice(&color[..inputs]),
        };
        let kind = if kind <= 5 {
            let triangles: Vec<[MeshVertex; 3]> = primitives
                .iter()
                .filter_map(|p| match p {
                    OldPrimitive::Triangle(v) => {
                        Some([vertex(&v[0]), vertex(&v[1]), vertex(&v[2])])
                    }
                    OldPrimitive::Patch { .. } => None,
                })
                .collect();
            ShadingKind::Triangles(triangles.into())
        } else {
            let patches: Vec<Patch> = primitives
                .iter()
                .filter_map(|p| match p {
                    OldPrimitive::Patch { points, colors } => {
                        let mut layout = [Point::default(); 16];
                        for i in 0..4 {
                            for j in 0..4 {
                                layout[4 * i + j] = points[4 * j + i];
                            }
                        }
                        Some(Patch {
                            points: layout,
                            colors: colors.map(|c| Components::from_slice(&c[..inputs])),
                        })
                    }
                    OldPrimitive::Triangle(_) => None,
                })
                .collect();
            ShadingKind::Patches(patches.into())
        };
        Some((primitives, kind))
    }
}

fn colorant(name: Name) -> Colorant {
    match name.as_bytes() {
        b"All" => Colorant::All,
        b"None" => Colorant::None,
        _ => Colorant::Named(name),
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

type OldVertex = (Point, [f64; 32]);

const ORDER: [usize; 16] = [0, 4, 8, 12, 13, 14, 15, 11, 7, 3, 2, 1, 5, 9, 10, 6];

impl Decoder<'_> {
    fn more(&self) -> bool {
        self.bit / 8 < self.data.len()
    }

    fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }

    fn bits(&mut self, n: usize) -> Option<u32> {
        let end = self
            .bit
            .checked_add(n)
            .filter(|n| n.div_ceil(8) <= self.data.len())?;
        let mut value = 0;
        while self.bit < end {
            value = (value << 1) | u32::from((self.data[self.bit / 8] >> (7 - self.bit % 8)) & 1);
            self.bit += 1;
        }
        Some(value)
    }

    fn flag(&mut self) -> Option<u8> {
        u8::try_from(self.bits(self.flag)? & 3).ok()
    }

    fn value(&mut self, bits: usize, index: usize) -> Option<f64> {
        let raw = self.bits(bits)?;
        let [lo, hi] = self.decode[index];
        let max = if bits == 32 {
            u32::MAX
        } else {
            (1u32 << bits) - 1
        };
        Some(lo + (hi - lo) * f64::from(raw) / f64::from(max))
    }

    fn point(&mut self) -> Option<Point> {
        Some(Point::new(
            self.value(self.coordinate, 0)?,
            self.value(self.coordinate, 1)?,
        ))
    }

    fn color(&mut self) -> Option<[f64; 32]> {
        let mut c = [0.0; 32];
        for (i, c) in c.iter_mut().enumerate().take(self.inputs) {
            *c = self.value(self.component, 2 + i)?;
        }
        Some(c)
    }

    fn vertex(&mut self) -> Option<OldVertex> {
        Some((self.point()?, self.color()?))
    }

    fn free_triangles(&mut self) -> Option<Vec<OldPrimitive>> {
        let mut out = Vec::new();
        let mut previous: Option<[OldVertex; 3]> = None;
        while self.more() {
            let flag = self.flag()?;
            let v = self.vertex()?;
            self.align();
            let vertices = match flag {
                0 => {
                    self.flag()?;
                    let b = self.vertex()?;
                    self.align();
                    self.flag()?;
                    let c = self.vertex()?;
                    self.align();
                    [v, b, c]
                }
                1 | 2 => {
                    let p = previous?;
                    [p[usize::from(flag == 1)], p[2], v]
                }
                _ => return None,
            };
            previous = Some(vertices);
            out.push(OldPrimitive::Triangle(vertices));
        }
        Some(out)
    }

    fn lattice(&mut self, row: usize) -> Option<Vec<OldPrimitive>> {
        if row < 2 {
            return None;
        }
        let mut out = Vec::new();
        let mut previous: Vec<OldVertex> = Vec::new();
        let mut current: Vec<OldVertex> = Vec::new();
        while self.more() {
            current.push(self.vertex()?);
            self.align();
            if current.len() == row {
                if !previous.is_empty() {
                    for i in 0..row - 1 {
                        out.push(OldPrimitive::Triangle([
                            previous[i],
                            previous[i + 1],
                            current[i],
                        ]));
                        out.push(OldPrimitive::Triangle([
                            previous[i + 1],
                            current[i],
                            current[i + 1],
                        ]));
                    }
                }
                std::mem::swap(&mut current, &mut previous);
                current.clear();
            }
        }
        current.is_empty().then_some(out)
    }

    fn patches(&mut self, coons: bool) -> Option<Vec<OldPrimitive>> {
        let mut out = Vec::new();
        let mut previous: Option<([Point; 16], [[f64; 32]; 4])> = None;
        while self.more() {
            let flag = usize::from(self.flag()?);
            let mut raw = [Point::default(); 16];
            let mut colors = [[0.0; 32]; 4];
            if flag != 0 {
                let (points, old) = previous?;
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
                *p = self.point()?;
            }
            for c in colors.iter_mut().skip(if flag == 0 { 0 } else { 2 }) {
                *c = self.color()?;
            }
            self.align();
            previous = Some((raw, colors));
            let mut points = [Point::default(); 16];
            for (i, &index) in ORDER.iter().enumerate() {
                points[index] = raw[i];
            }
            if coons {
                let mix = |a: Point, b: Point, t: f64| {
                    Point::new(a.x * (1.0 - t) + b.x * t, a.y * (1.0 - t) + b.y * t)
                };
                for (i, j) in [(1_usize, 1_usize), (1, 2), (2, 1), (2, 2)] {
                    let u = i as f64 / 3.0;
                    let v = j as f64 / 3.0;
                    let sc = mix(points[i], points[12 + i], v);
                    let sd = mix(points[4 * j], points[4 * j + 3], u);
                    let sb = mix(
                        mix(points[0], points[3], u),
                        mix(points[12], points[15], u),
                        v,
                    );
                    points[j * 4 + i] = Point::new(sc.x + sd.x - sb.x, sc.y + sd.y - sb.y);
                }
            }
            out.push(OldPrimitive::Patch { points, colors });
        }
        Some(out)
    }
}
