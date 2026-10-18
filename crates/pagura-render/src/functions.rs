//! Bounded PDF functions (7.10). Input and output clipping is part of evaluation.

use crate::color::MAX_COMPONENTS;
use pagura_core::{Dictionary, Error, Limits, ObjectId, Result, ResultExt, Value};
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
        result.in_object_opt(object)
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
        if domain.len() != inputs
            || !(1..=MAX_COMPONENTS).contains(&inputs)
            || !(1..=MAX_COMPONENTS).contains(&outputs)
        {
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
        Ok(Self {
            domain,
            range,
            outputs,
            body,
            limits: document.limits(),
            object: value.as_reference(),
        })
    }
    /// The identity function `[0 1] → [0 1]`, for `/Identity` transfers.
    fn identity(limits: Limits) -> Self {
        Self {
            domain: vec![[0.0, 1.0]],
            range: None,
            outputs: 1,
            body: Body::Exponential {
                low: vec![0.0],
                high: vec![1.0],
                power: 1.0,
            },
            limits,
            object: None,
        }
    }
    pub(crate) fn outputs(&self) -> usize {
        self.outputs
    }
    pub(crate) fn covers_domain(&self, domain: &[[f64; 2]]) -> bool {
        domain.len() == self.domain.len()
            && domain
                .iter()
                .zip(&self.domain)
                .all(|(a, b)| a[0] >= b[0] && a[1] <= b[1])
    }
    /// Evaluates the function, writing its `outputs()` values into
    /// `output`, without allocating. `work` counts evaluation steps of one
    /// page compilation or rendering.
    ///
    /// # Errors
    /// `Syntax` for non-finite inputs or undefined arithmetic, `Limit` when
    /// `work` exceeds the budget.
    pub(crate) fn evaluate_into(
        &self,
        inputs: &[f64],
        output: &mut [f64],
        work: &mut usize,
    ) -> Result<()> {
        debug_assert_eq!(
            output.len(),
            self.outputs,
            "output buffer fits the function"
        );
        self.run(inputs, output, 0, work).in_object_opt(self.object)
    }
    /// Evaluates a one-in, one-out function (transfers, mask transfers,
    /// black generation and undercolour removal).
    pub(crate) fn evaluate_scalar(&self, input: f64, work: &mut usize) -> Result<f64> {
        let mut output = [0.0];
        self.evaluate_into(&[input], &mut output, work)?;
        Ok(output[0])
    }
    fn run(
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
        // Parsing bounds the dimension by MAX_COMPONENTS.
        let mut clamped = [0.0; MAX_COMPONENTS];
        let input = clamped
            .get_mut(..inputs.len())
            .ok_or(Error::syntax("invalid function inputs"))?;
        for ((out, &v), r) in input.iter_mut().zip(inputs).zip(&self.domain) {
            *out = v.clamp(r[0], r[1]);
        }
        match &self.body {
            Body::Exponential { low, high, power } => {
                let x = input
                    .first()
                    .copied()
                    .ok_or(Error::syntax("missing function input"))?
                    .powf(*power);
                for ((out, &a), &b) in output.iter_mut().zip(low).zip(high) {
                    *out = a + x * (b - a);
                }
            }
            Body::Stitch {
                functions,
                bounds,
                encode,
            } => {
                let x = input
                    .first()
                    .copied()
                    .ok_or(Error::syntax("missing function input"))?;
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
                    .run(&[x], output, depth + 1, work)?;
            }
            Body::Calculator(calculator) => {
                calculator.evaluate(input, output, work, &self.limits)?;
            }
            Body::Sampled(sampled) => {
                sampled.evaluate(input, &self.domain, output, work, &self.limits)?;
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
    crate::numbers::list(document, value, f64::MAX)
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

/// Transfer functions for the red, green and blue components (10.5);
/// one function given for all three is shared.
pub(crate) type Transfer = Arc<[Arc<Function>; 3]>;
pub(crate) fn transfer(document: &Document, value: &Value) -> Result<Option<Transfer>> {
    let resolved = document.resolve(value)?;
    if resolved
        .as_name()
        .is_some_and(|n| n == b"Identity" || n == b"Default")
    {
        return Ok(None);
    }
    let Some(array) = resolved.as_array() else {
        let function = Arc::new(Function::parse(document, value, 1, 1)?);
        return Ok(Some(Arc::new([
            Arc::clone(&function),
            Arc::clone(&function),
            function,
        ])));
    };
    let [red, green, blue, gray] = <&[Value; 4]>::try_from(array)
        .map_err(|_| Error::syntax("transfer array requires four functions"))?;
    let read = |value: &Value| -> Result<Option<Arc<Function>>> {
        if document
            .resolve(value)?
            .as_name()
            .is_some_and(|n| n == b"Identity")
        {
            Ok(None)
        } else {
            Function::parse(document, value, 1, 1).map(|f| Some(Arc::new(f)))
        }
    };
    let channels = [read(red)?, read(green)?, read(blue)?];
    // The fourth (grey) transfer is validated but has no effect on RGB output.
    read(gray)?;
    // 10.5: Identity on every applied component is the default transfer, so
    // transparency classification (11.7.5.2) treats it as no transfer.
    if channels.iter().all(Option::is_none) {
        return Ok(None);
    }
    let identity = || Arc::new(Function::identity(document.limits()));
    Ok(Some(Arc::new(
        channels.map(|channel| channel.unwrap_or_else(identity)),
    )))
}
