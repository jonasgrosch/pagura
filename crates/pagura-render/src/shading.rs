//! Analytic shadings and function sampling (8.7.4.5.2–8.7.4.5.4).
//!
//! The continuous-tone device samples pixel centres by default. AntiAlias
//! requests a deterministic 2x2 box filter. Sampling uses global coordinates,
//! so changing the tile partition never changes the function arguments.

mod mesh;

use crate::color::{self, Space};
use crate::functions::Function;
use crate::icc::ColorContext;
use pagura_core::{Dictionary, Error, Rect, Result, Value};
use pagura_raster::{Point, Transform};

#[derive(Debug)]
pub(crate) struct Shading {
    pub(crate) space: Space,
    pub(crate) bbox: Option<Rect>,
    pub(crate) bbox_path: Option<pagura_raster::Path>,
    pub(crate) antialias: bool,
    pub(crate) background: Option<[f64; 32]>,
    functions: Functions,
    geometry: Geometry,
}
#[derive(Debug)]
enum Geometry {
    Mesh(Box<mesh::Mesh>),
    Function {
        domain: [f64; 4],
        inverse: Transform,
    },
    Axial {
        start: Point,
        delta: Point,
        domain: [f64; 2],
        extend: [bool; 2],
    },
    Radial {
        start: Point,
        delta: Point,
        radius: f64,
        change: f64,
        domain: [f64; 2],
        extend: [bool; 2],
    },
}
#[derive(Debug)]
enum Functions {
    Identity,
    Vector(Box<Function>),
    Components(Vec<Function>),
}

impl Shading {
    pub(crate) fn mesh_bytes(&self) -> usize {
        match &self.geometry {
            Geometry::Mesh(mesh) => mesh.retained_bytes(),
            _ => 0,
        }
    }

    pub(crate) fn parse(
        document: &ColorContext<'_>,
        resources: &Dictionary,
        value: &Value,
    ) -> Result<Self> {
        let d = document.resolve_dictionary(value)?;
        let kind = d
            .get(b"ShadingType")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_integer)
            .ok_or(Error::syntax("shading requires ShadingType"))?;
        if !(1..=7).contains(&kind) {
            return Err(Error::syntax("invalid ShadingType"));
        }
        let space = color::blending(
            document,
            resources,
            d.get(b"ColorSpace")
                .ok_or(Error::syntax("shading requires ColorSpace"))?,
        )?;
        if matches!(space, Space::Pattern(_))
            || (kind < 4 && matches!(space, Space::Indexed { .. }))
            || (matches!(space, Space::Indexed { .. }) && d.contains_key(b"Function"))
        {
            return Err(Error::syntax("analytic shading cannot use Indexed"));
        }
        let components = space.components();
        if components > 32 {
            return Err(Error::limit(
                "shading component count exceeds implementation limit (Annex C)",
            ));
        }
        let bbox = d
            .get(b"BBox")
            .map(|v| {
                let n = numbers::<4>(document, v)?;
                let b = Rect::from_corners(n[0], n[1], n[2], n[3])
                    .ok_or(Error::syntax("invalid shading BBox"))?;
                Ok(b)
            })
            .transpose()?;
        let bbox_path = bbox.map(|b| {
            let mut path = pagura_raster::PathBuilder::new();
            path.rect(b.x0(), b.y0(), b.width(), b.height());
            path.finish()
        });
        let antialias = d
            .get(b"AntiAlias")
            .map(|v| {
                document
                    .resolve(v)?
                    .as_bool()
                    .ok_or(Error::syntax("shading AntiAlias must be boolean"))
            })
            .transpose()?
            .unwrap_or(false);
        let background = d
            .get(b"Background")
            .map(|v| {
                let values = document
                    .resolve(v)?
                    .as_array()
                    .filter(|v| v.len() == components)
                    .ok_or(Error::syntax("wrong shading Background dimension"))?;
                let mut background = [0.0; 32];
                for (out, v) in background.iter_mut().zip(values) {
                    *out = document
                        .resolve(v)?
                        .as_number()
                        .ok_or(Error::syntax("shading Background must be numeric"))?;
                }
                Ok(background)
            })
            .transpose()?;
        if kind >= 4 {
            let mesh = mesh::Mesh::parse(document, d, value, kind, components)?;
            return Ok(Self {
                space,
                bbox,
                bbox_path,
                antialias,
                background,
                functions: Functions::Identity,
                geometry: Geometry::Mesh(Box::new(mesh)),
            });
        }
        let (geometry, domain) = geometry(document, d, kind)?;
        let functions = Functions::parse(
            document,
            d.get(b"Function")
                .ok_or(Error::syntax("shading requires Function"))?,
            &domain,
            components,
        )?;
        Ok(Self {
            space,
            bbox,
            bbox_path,
            antialias,
            background,
            functions,
            geometry,
        })
    }
    pub(crate) fn sample(
        &self,
        point: Point,
        output: &mut [f64; 32],
        work: &mut usize,
        raster_work: &mut usize,
    ) -> Result<bool> {
        if let Geometry::Mesh(mesh) = &self.geometry {
            if !mesh.sample(point, output, raster_work)? {
                return Ok(false);
            }
            let input = *output;
            let count = if matches!(mesh.functions, Functions::Identity) {
                self.space.components()
            } else {
                1
            };
            mesh.functions.evaluate(
                &input[..count],
                &mut output[..self.space.components()],
                work,
            )?;
            return Ok(true);
        }
        let (input, count) = match self.geometry {
            Geometry::Mesh(_) => {
                return Err(Error::invalid_argument("invalid analytic shading geometry"));
            }
            Geometry::Function { domain, inverse } => {
                let p = point * inverse;
                if p.x < domain[0] || p.x > domain[1] || p.y < domain[2] || p.y > domain[3] {
                    return Ok(false);
                }
                ([p.x, p.y], 2)
            }
            Geometry::Axial {
                start,
                delta,
                domain,
                extend,
            } => {
                let denominator = delta.x * delta.x + delta.y * delta.y;
                if denominator == 0.0 {
                    return Ok(false);
                }
                let t =
                    ((point.x - start.x) * delta.x + (point.y - start.y) * delta.y) / denominator;
                let Some(t) = parameter(t, extend) else {
                    return Ok(false);
                };
                ([domain[0] + t * (domain[1] - domain[0]), 0.0], 1)
            }
            Geometry::Radial {
                start,
                delta,
                radius,
                change,
                domain,
                extend,
            } => {
                if radius == 0.0 && change == 0.0 {
                    return Ok(false);
                }
                let x = point.x - start.x;
                let y = point.y - start.y;
                let a = delta.x * delta.x + delta.y * delta.y - change * change;
                let b = -2.0 * (x * delta.x + y * delta.y + radius * change);
                let c = x * x + y * y - radius * radius;
                let roots = roots(a, b, c);
                let t = roots
                    .into_iter()
                    .filter(|t| t.is_finite() && radius + change * t >= 0.0)
                    .filter_map(|t| parameter(t, extend))
                    .max_by(f64::total_cmp);
                let Some(t) = t else { return Ok(false) };
                ([domain[0] + t * (domain[1] - domain[0]), 0.0], 1)
            }
        };
        self.functions.evaluate(
            &input[..count],
            &mut output[..self.space.components()],
            work,
        )?;
        Ok(true)
    }
}

fn geometry(
    document: &ColorContext<'_>,
    d: &Dictionary,
    kind: i64,
) -> Result<(Geometry, Vec<[f64; 2]>)> {
    if kind == 1 {
        let domain = optional_numbers(document, d, b"Domain", [0.0, 1.0, 0.0, 1.0])?;
        if domain[0] >= domain[1] || domain[2] >= domain[3] {
            return Err(Error::syntax("invalid shading Domain"));
        }
        let m = optional_numbers(document, d, b"Matrix", [1.0, 0.0, 0.0, 1.0, 0.0, 0.0])?;
        let inverse = Transform::new(m[0], m[1], m[2], m[3], m[4], m[5])
            .inverse()
            .ok_or(Error::unsupported(
                crate::features::SHADING_TRANSFORM,
                "singular shading Matrix",
            ))?;
        return Ok((
            Geometry::Function { domain, inverse },
            vec![[domain[0], domain[1]], [domain[2], domain[3]]],
        ));
    }
    let domain = optional_numbers(document, d, b"Domain", [0.0, 1.0])?;
    if domain[0] >= domain[1] {
        return Err(Error::syntax("invalid shading Domain"));
    }
    let extend = d
        .get(b"Extend")
        .map(|v| {
            let values = document
                .resolve(v)?
                .as_array()
                .filter(|v| v.len() == 2)
                .ok_or(Error::syntax("shading Extend requires two flags"))?;
            let mut result = [false; 2];
            for (out, v) in result.iter_mut().zip(values) {
                *out = document
                    .resolve(v)?
                    .as_bool()
                    .ok_or(Error::syntax("shading Extend must be boolean"))?;
            }
            Ok(result)
        })
        .transpose()?
        .unwrap_or([false; 2]);
    let coords = d
        .get(b"Coords")
        .ok_or(Error::syntax("shading requires Coords"))?;
    let geometry = if kind == 2 {
        let c = numbers::<4>(document, coords)?;
        Geometry::Axial {
            start: Point::new(c[0], c[1]),
            delta: Point::new(c[2] - c[0], c[3] - c[1]),
            domain,
            extend,
        }
    } else {
        let c = numbers::<6>(document, coords)?;
        if c[2] < 0.0 || c[5] < 0.0 {
            return Err(Error::syntax("negative shading radius"));
        }
        Geometry::Radial {
            start: Point::new(c[0], c[1]),
            delta: Point::new(c[3] - c[0], c[4] - c[1]),
            radius: c[2],
            change: c[5] - c[2],
            domain,
            extend,
        }
    };
    Ok((geometry, vec![domain]))
}
impl Functions {
    fn parse(
        document: &ColorContext<'_>,
        value: &Value,
        domain: &[[f64; 2]],
        outputs: usize,
    ) -> Result<Self> {
        let read = |v, outputs| {
            let f = Function::parse(document, v, domain.len(), outputs)?;
            if !f.covers_domain(domain) {
                return Err(Error::syntax("function domain does not cover shading"));
            }
            Ok(f)
        };
        if let Some(values) = document.resolve(value)?.as_array() {
            if values.len() != outputs {
                return Err(Error::syntax("wrong shading Function array dimension"));
            }
            Ok(Self::Components(
                values
                    .iter()
                    .map(|v| read(v, 1))
                    .collect::<Result<Vec<_>>>()?,
            ))
        } else {
            Ok(Self::Vector(Box::new(read(value, outputs)?)))
        }
    }
    fn evaluate(&self, input: &[f64], output: &mut [f64], work: &mut usize) -> Result<()> {
        match self {
            Self::Identity => {
                output.copy_from_slice(input);
                Ok(())
            }
            Self::Vector(f) => f.evaluate_into(input, output, work),
            Self::Components(functions) => {
                for (function, out) in functions.iter().zip(output) {
                    function.evaluate_into(input, std::slice::from_mut(out), work)?;
                }
                Ok(())
            }
        }
    }
}
fn parameter(t: f64, extend: [bool; 2]) -> Option<f64> {
    (t.is_finite() && (t >= 0.0 || extend[0]) && (t <= 1.0 || extend[1])).then(|| t.clamp(0.0, 1.0))
}
/// Stable quadratic evaluation; the linear case avoids division by zero.
fn roots(a: f64, b: f64, c: f64) -> [f64; 2] {
    if a == 0.0 {
        return if b == 0.0 { [f64::NAN; 2] } else { [-c / b; 2] };
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return [f64::NAN; 2];
    }
    let q = -0.5 * (b + discriminant.sqrt().copysign(b));
    if q == 0.0 {
        [-b / (2.0 * a); 2]
    } else {
        [q / a, c / q]
    }
}
fn numbers<const N: usize>(document: &ColorContext<'_>, value: &Value) -> Result<[f64; N]> {
    crate::numbers::array(document, value, crate::numbers::GEOMETRY)
}
fn optional_numbers<const N: usize>(
    document: &ColorContext<'_>,
    d: &Dictionary,
    key: &[u8],
    default: [f64; N],
) -> Result<[f64; N]> {
    d.get(key)
        .map(|v| numbers(document, v))
        .transpose()
        .map(|v| v.unwrap_or(default))
}
