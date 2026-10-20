//! Bounded PDF functions (7.10). Input and output clipping is part of evaluation.

use pagura_core::{Dictionary, Error, Limits, ObjectId, Result, Value};
use pagura_document::Document;
use std::sync::Arc;
mod calculator;
mod sampled;

#[derive(Debug, Clone)]
pub(crate) struct Function {
    domain: Vec<[f64; 2]>,
    range: Option<Vec<[f64; 2]>>,
    outputs: usize,
    body: Body,
    limits: Limits,
    object: Option<ObjectId>,
    /// A sampled body on the unit domain whose grid needs neither Encode nor
    /// Decode arithmetic (`Sampled::unit_byte_grid`).
    unit_bytes: bool,
}
#[derive(Debug, Clone)]
enum Body {
    Exponential {
        low: Vec<f64>,
        high: Vec<f64>,
        power: f64,
    },
    Stitch {
        functions: Vec<Function>,
        bounds: Vec<f64>,
        encode: Vec<[f64; 2]>,
    },
    Sampled(sampled::Sampled),
    Calculator(calculator::Calculator),
}

impl Function {
    pub(crate) fn parse(
        document: &Document,
        value: &Value,
        inputs: usize,
        outputs: usize,
    ) -> Result<Self> {
        Self::read(document, value, inputs, outputs, &mut Vec::new(), &mut 0)
    }
    fn read(
        document: &Document,
        value: &Value,
        inputs: usize,
        outputs: usize,
        ancestors: &mut Vec<ObjectId>,
        work: &mut usize,
    ) -> Result<Self> {
        if *work >= document.limits().value_nodes {
            return Err(Error::limit("function graph budget exceeded"));
        }
        *work += 1;
        if ancestors.len() >= document.limits().depth {
            return Err(Error::limit("function nesting exceeded"));
        }
        let object = value.as_reference();
        if let Some(id) = object {
            if ancestors.contains(&id) {
                return Err(Error::cycle("recursive PDF function").in_object(id));
            }
            ancestors.push(id);
        }
        let result = Self::dictionary(document, value, inputs, outputs, ancestors, work);
        if object.is_some() {
            ancestors.pop();
        }
        result.map_err(|e| {
            if let Some(id) = object {
                e.in_object(id)
            } else {
                e
            }
        })
    }
    fn dictionary(
        document: &Document,
        value: &Value,
        inputs: usize,
        outputs: usize,
        ancestors: &mut Vec<ObjectId>,
        work: &mut usize,
    ) -> Result<Self> {
        let dictionary = document.resolve_dictionary(value)?;
        let domain = pairs(document, dictionary, b"Domain", None)?;
        if domain.len() != inputs || inputs == 0 || outputs == 0 {
            return Err(Error::syntax("wrong function input dimension"));
        }
        let range = if dictionary.contains_key(b"Range") {
            Some(pairs(document, dictionary, b"Range", None)?)
        } else {
            None
        };
        if range.as_ref().is_some_and(|v| v.len() != outputs) {
            return Err(Error::syntax("wrong function output dimension"));
        }
        let kind = dictionary
            .get(b"FunctionType")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_integer)
            .ok_or(Error::syntax("function has no FunctionType"))?;
        let body = match kind {
            0 => Body::Sampled(sampled::Sampled::parse(
                document,
                value,
                dictionary,
                inputs,
                outputs,
                range
                    .as_deref()
                    .ok_or(Error::syntax("sampled function requires Range"))?,
            )?),
            2 => exponential(document, dictionary, inputs, outputs, &domain)?,
            3 => {
                if inputs != 1 {
                    return Err(Error::syntax("stitching function requires one input"));
                }
                let values = dictionary
                    .get(b"Functions")
                    .map(|v| document.resolve(v))
                    .transpose()?
                    .and_then(Value::as_array)
                    .filter(|v| !v.is_empty())
                    .ok_or(Error::syntax("stitching function requires Functions"))?;
                if values.len() > document.limits().value_nodes {
                    return Err(Error::limit("stitching function count exceeded"));
                }
                let bounds = numbers(document, dictionary, b"Bounds", None)?;
                let encode = pairs_unordered(document, dictionary, b"Encode", None)?;
                if bounds.len() + 1 != values.len() || encode.len() != values.len() {
                    return Err(Error::syntax("wrong stitching bounds or encode count"));
                }
                let [min, max] = *domain
                    .first()
                    .ok_or(Error::syntax("missing function domain"))?;
                if (values.len() > 1 && min == max)
                    || bounds.iter().any(|&v| v < min || v > max)
                    || bounds.windows(2).any(|v| v[0] >= v[1])
                {
                    return Err(Error::syntax(
                        "stitching bounds outside domain or unordered",
                    ));
                }
                let mut functions = Vec::new();
                for value in values {
                    if ancestors.len() >= document.limits().depth {
                        return Err(Error::limit("function nesting exceeded"));
                    }
                    // Direct dictionaries need a depth entry too; object zero is
                    // never a valid indirect identity and is not used for cycles.
                    ancestors.push(ObjectId::new(0, 0));
                    let function = Self::read(document, value, 1, outputs, ancestors, work)?;
                    ancestors.pop();
                    functions.push(function);
                }
                Body::Stitch {
                    functions,
                    bounds,
                    encode,
                }
            }
            4 => calculator(document, value, range.is_some())?,
            _ => return Err(Error::syntax("invalid FunctionType")),
        };
        let unit_bytes =
            domain == [[0.0, 1.0]] && matches!(&body, Body::Sampled(s) if s.unit_byte_grid());
        Ok(Self {
            domain,
            range,
            outputs,
            body,
            limits: document.limits(),
            object: value.as_reference(),
            unit_bytes,
        })
    }
    pub(crate) fn evaluate(&self, inputs: &[f64]) -> Result<Vec<f64>> {
        self.evaluate_with_work(inputs, &mut 0)
    }
    pub(crate) fn covers_domain(&self, domain: &[[f64; 2]]) -> bool {
        domain.len() == self.domain.len()
            && domain
                .iter()
                .zip(&self.domain)
                .all(|(a, b)| a[0] >= b[0] && a[1] <= b[1])
    }
    /// Evaluate into a caller's buffer, without allocating per sample.
    pub(crate) fn evaluate_into(
        &self,
        inputs: &[f64],
        output: &mut [f64],
        work: &mut usize,
    ) -> Result<()> {
        self.fill_output(inputs, output, work)
            .map_err(|error| self.object.map_or(error.clone(), |id| error.in_object(id)))
    }
    fn fill_output(&self, inputs: &[f64], output: &mut [f64], work: &mut usize) -> Result<()> {
        if inputs.len() != self.domain.len() || inputs.iter().any(|v| !v.is_finite()) {
            return Err(Error::syntax("invalid function inputs"));
        }
        if output.len() != self.outputs {
            return Err(Error::invalid_argument("wrong function output buffer"));
        }
        if let Body::Exponential { low, high, power } = &self.body {
            if *work >= self.limits.operations {
                return Err(Error::limit("function evaluation budget exceeded"));
            }
            *work += 1;
            let [value] = inputs else {
                return Err(Error::syntax("wrong function input count"));
            };
            let range = self
                .domain
                .first()
                .ok_or(Error::syntax("missing function domain"))?;
            let value = value.clamp(range[0], range[1]).powf(*power);
            for (i, ((out, &low), &high)) in output.iter_mut().zip(low).zip(high).enumerate() {
                *out = low + value * (high - low);
                if !out.is_finite() {
                    return Err(Error::syntax(
                        "function arithmetic is undefined or overflows",
                    ));
                }
                if let Some(range) = self.range.as_ref().and_then(|r| r.get(i)) {
                    *out = out.clamp(range[0], range[1]);
                }
            }
            return Ok(());
        }
        self.run_into(inputs, output, 0, work)
    }
    pub(crate) fn evaluate_with_work(&self, inputs: &[f64], work: &mut usize) -> Result<Vec<f64>> {
        self.run(inputs, 0, work).map_err(|e| {
            if let Some(id) = self.object {
                e.in_object(id)
            } else {
                e
            }
        })
    }
    fn run(&self, inputs: &[f64], depth: usize, work: &mut usize) -> Result<Vec<f64>> {
        let mut output = vec![0.0; self.outputs];
        self.run_into(inputs, &mut output, depth, work)?;
        Ok(output)
    }
    /// Evaluates into `output`, which holds exactly `self.outputs` values.
    fn run_into(
        &self,
        inputs: &[f64],
        output: &mut [f64],
        depth: usize,
        work: &mut usize,
    ) -> Result<()> {
        if depth >= self.limits.depth || *work >= self.limits.operations {
            return Err(Error::limit("function evaluation budget exceeded"));
        }
        *work += 1;
        if inputs.len() != self.domain.len() || inputs.iter().any(|v| !v.is_finite()) {
            return Err(Error::syntax("invalid function inputs"));
        }
        // Exponential, stitching and unit-grid sampled functions have one input.
        let first = || -> Result<f64> {
            let value = inputs
                .first()
                .copied()
                .ok_or(Error::syntax("missing function input"))?;
            let range = self
                .domain
                .first()
                .ok_or(Error::syntax("missing function domain"))?;
            Ok(value.clamp(range[0], range[1]))
        };
        match &self.body {
            Body::Exponential { low, high, power } => {
                let x = first()?.powf(*power);
                for ((out, &a), &b) in output.iter_mut().zip(low).zip(high) {
                    *out = a + x * (b - a);
                }
            }
            Body::Stitch {
                functions,
                bounds,
                encode,
            } => {
                let x = first()?;
                // The first degenerate interval owns its endpoint (7.10.4).
                let index = if bounds.first() == Some(&x)
                    && self.domain.first().is_some_and(|d| d[0] == x)
                {
                    0
                } else {
                    bounds.partition_point(|&b| x >= b)
                };
                let domain = *self
                    .domain
                    .first()
                    .ok_or(Error::syntax("missing function domain"))?;
                let low = if index == 0 {
                    domain[0]
                } else {
                    *bounds
                        .get(index - 1)
                        .ok_or(Error::syntax("invalid stitching index"))?
                };
                let high = bounds.get(index).copied().unwrap_or(domain[1]);
                let encode = *encode
                    .get(index)
                    .ok_or(Error::syntax("invalid stitching encode"))?;
                let x = interpolate(x, [low, high], encode);
                functions
                    .get(index)
                    .ok_or(Error::syntax("invalid stitching index"))?
                    .run_into(&[x], output, depth + 1, work)?;
            }
            Body::Calculator(calculator) => {
                let input: Vec<_> = inputs
                    .iter()
                    .zip(&self.domain)
                    .map(|(&v, r)| v.clamp(r[0], r[1]))
                    .collect();
                let result = calculator.evaluate(&input, self.outputs, work, &self.limits)?;
                if result.len() != output.len() {
                    return Err(Error::syntax(
                        "function arithmetic is undefined or overflows",
                    ));
                }
                output.copy_from_slice(&result);
            }
            Body::Sampled(sampled) if self.unit_bytes => {
                sampled.evaluate_unit_bytes(first()?, output, work, &self.limits)?;
            }
            Body::Sampled(sampled) => {
                sampled.evaluate_into(inputs, &self.domain, output, work, &self.limits)?;
            }
        }
        if output.iter().any(|v| !v.is_finite()) {
            return Err(Error::syntax(
                "function arithmetic is undefined or overflows",
            ));
        }
        if let Some(range) = &self.range {
            for (v, r) in output.iter_mut().zip(range) {
                *v = v.clamp(r[0], r[1]);
            }
        }
        Ok(())
    }
}

fn calculator(document: &Document, value: &Value, has_range: bool) -> Result<Body> {
    if !has_range {
        return Err(Error::syntax("calculator function requires Range"));
    }
    let id = value.as_reference().ok_or(Error::syntax(
        "calculator function must be a stream reference",
    ))?;
    let object = document.object(id)?;
    if !object.is_stream() {
        return Err(Error::syntax("calculator function must be a stream"));
    }
    Ok(Body::Calculator(calculator::Calculator::parse(
        &document.stream_data(object)?,
        &document.limits(),
        id,
    )?))
}

fn exponential(
    document: &Document,
    dictionary: &Dictionary,
    inputs: usize,
    outputs: usize,
    domain: &[[f64; 2]],
) -> Result<Body> {
    if inputs != 1 {
        return Err(Error::syntax("exponential function requires one input"));
    }
    let low = numbers(document, dictionary, b"C0", Some(&[0.0]))?;
    let high = numbers(document, dictionary, b"C1", Some(&[1.0]))?;
    let power = dictionary
        .get(b"N")
        .map(|v| document.resolve(v))
        .transpose()?
        .and_then(Value::as_number)
        .ok_or(Error::syntax("exponential function has no N"))?;
    let [min, max] = *domain
        .first()
        .ok_or(Error::syntax("missing function domain"))?;
    if (power.fract() != 0.0 && min < 0.0) || (power < 0.0 && min <= 0.0 && max >= 0.0) {
        return Err(Error::syntax(
            "exponential domain includes undefined inputs",
        ));
    }
    if low.len() != outputs || high.len() != outputs {
        return Err(Error::syntax("wrong exponential function output dimension"));
    }
    Ok(Body::Exponential { low, high, power })
}

pub(super) fn interpolate(value: f64, from: [f64; 2], to: [f64; 2]) -> f64 {
    if from[0] == from[1] {
        return to[0];
    }
    to[0] + ((value - from[0]) / (from[1] - from[0])) * (to[1] - to[0])
}
fn numbers(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    default: Option<&[f64]>,
) -> Result<Vec<f64>> {
    let Some(value) = dictionary.get(key) else {
        return default
            .map(<[f64]>::to_vec)
            .ok_or(Error::syntax("missing function parameter"));
    };
    let array = document
        .resolve(value)?
        .as_array()
        .ok_or(Error::syntax("function parameter must be an array"))?;
    array
        .iter()
        .map(|v| {
            document
                .resolve(v)?
                .as_number()
                .ok_or(Error::syntax("function parameter must be numeric"))
        })
        .collect()
}
fn pairs_unordered(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    default: Option<&[[f64; 2]]>,
) -> Result<Vec<[f64; 2]>> {
    if !dictionary.contains_key(key) {
        return default
            .map(<[[f64; 2]]>::to_vec)
            .ok_or(Error::syntax("missing function parameter"));
    }
    let values = numbers(document, dictionary, key, None)?;
    let (pairs, remainder) = values.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(Error::syntax("function pairs require even length"));
    }
    Ok(pairs.to_vec())
}
fn pairs(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    default: Option<&[[f64; 2]]>,
) -> Result<Vec<[f64; 2]>> {
    let values = pairs_unordered(document, dictionary, key, default)?;
    if values.iter().any(|r| r[0] > r[1]) {
        return Err(Error::syntax("reversed function range"));
    }
    Ok(values)
}

pub(crate) type Transfer = Arc<[Function; 3]>;
pub(crate) fn transfer(document: &Document, value: &Value) -> Result<Option<Transfer>> {
    let resolved = document.resolve(value)?;
    if resolved
        .as_name()
        .is_some_and(|n| n == b"Identity" || n == b"Default")
    {
        return Ok(None);
    }
    let functions = if let Some(array) = resolved.as_array() {
        if array.len() != 4 {
            return Err(Error::syntax("transfer array requires four functions"));
        }
        // 10.5: Identity on every device component is the default transfer.
        // Preserve that identity in transparency classification (11.7.5.2),
        // rather than treating a syntactic array as a nondefault function.
        let mut identity = true;
        for value in array {
            identity &= document
                .resolve(value)?
                .as_name()
                .is_some_and(|n| n == b"Identity");
        }
        if identity {
            return Ok(None);
        }
        let values = array
            .iter()
            .map(|v| {
                if document
                    .resolve(v)?
                    .as_name()
                    .is_some_and(|n| n == b"Identity")
                {
                    Ok(Function {
                        domain: vec![[0.0, 1.0]],
                        range: None,
                        outputs: 1,
                        body: Body::Exponential {
                            low: vec![0.0],
                            high: vec![1.0],
                            power: 1.0,
                        },
                        limits: document.limits(),
                        object: None,
                        unit_bytes: false,
                    })
                } else {
                    Function::parse(document, v, 1, 1)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        [
            values
                .first()
                .cloned()
                .ok_or(Error::syntax("missing transfer"))?,
            values
                .get(1)
                .cloned()
                .ok_or(Error::syntax("missing transfer"))?,
            values
                .get(2)
                .cloned()
                .ok_or(Error::syntax("missing transfer"))?,
        ]
    } else {
        let function = Function::parse(document, value, 1, 1)?;
        [function.clone(), function.clone(), function]
    };
    Ok(Some(Arc::new(functions)))
}
