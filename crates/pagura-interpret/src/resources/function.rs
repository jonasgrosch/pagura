//! PDF function dictionaries and streams read into `pagura_color` functions
//! (7.10). Validation follows the order and messages of the reading rules
//! below; the colour crate's constructors then validate the parameters for
//! evaluation.

use super::numbers::{entry, list};
use crate::model::Transfer;
use pagura_color::{
    CalculatorFunction, ExponentialFunction, Function, SampledFunction, StitchingFunction,
};
use pagura_core::{Dictionary, Error, ObjectId, Position, Value};
use pagura_document::Document;
use pagura_document::source::ReadResult;
use std::sync::Arc;

/// A function and the domain it was read with, for coverage checks.
#[derive(Debug, Clone)]
pub(crate) struct ReadFunction {
    /// The function.
    pub(crate) function: Arc<Function>,
    /// `Domain`, one interval per input.
    pub(crate) domain: Vec<[f64; 2]>,
}

impl ReadFunction {
    /// Whether the function's domain includes every interval of `domain`.
    pub(crate) fn covers(&self, domain: &[[f64; 2]]) -> bool {
        domain.len() == self.domain.len()
            && domain
                .iter()
                .zip(&self.domain)
                .all(|(a, b)| a[0] >= b[0] && a[1] <= b[1])
    }
}

/// Reads the function `value` with `inputs` inputs and `outputs` outputs.
///
/// # Errors
///
/// `Syntax` for malformed dictionaries and streams, `Limit` for budgets,
/// `Cycle` for recursive stitching functions, and the errors of the
/// function constructors, in the function's object.
pub(crate) fn read(
    document: &Document,
    value: &Value,
    inputs: usize,
    outputs: usize,
) -> ReadResult<ReadFunction> {
    Reader {
        document,
        ancestors: Vec::new(),
        work: 0,
    }
    .read(value, inputs, outputs)
}

/// Reads a single-input, single-output function, as transfer, black
/// generation and undercolour removal entries use.
///
/// # Errors
///
/// As [`read`].
pub(crate) fn scalar(document: &Document, value: &Value) -> ReadResult<Arc<Function>> {
    read(document, value, 1, 1).map(|read| read.function)
}

/// Reads a transfer function entry (`TR`, `TR2`, 10.5): `None` for the
/// identity and the device default.
///
/// # Errors
///
/// As [`read`], and `Syntax` for an array that does not hold four entries.
pub(crate) fn transfer(document: &Document, value: &Value) -> ReadResult<Option<Arc<Transfer>>> {
    let resolved = document.resolve(value)?;
    if resolved
        .as_name()
        .is_some_and(|n| n == b"Identity" || n == b"Default")
    {
        return Ok(None);
    }
    let Some(array) = resolved.as_array() else {
        return Ok(Some(Arc::new(Transfer::All(scalar(document, value)?))));
    };
    if array.len() != 4 {
        return Err(Error::syntax("transfer array requires four functions").into());
    }
    // 10.5: Identity on every component is the default transfer.
    let mut identity = true;
    for value in array {
        identity &= is_identity(document, value)?;
    }
    if identity {
        return Ok(None);
    }
    let mut functions: [Option<Arc<Function>>; 4] = [None, None, None, None];
    for (function, value) in functions.iter_mut().zip(array) {
        if !is_identity(document, value)? {
            *function = Some(scalar(document, value)?);
        }
    }
    Ok(Some(Arc::new(Transfer::PerComponent(functions))))
}

fn is_identity(document: &Document, value: &Value) -> ReadResult<bool> {
    Ok(document
        .resolve(value)?
        .as_name()
        .is_some_and(|n| n == b"Identity"))
}

struct Reader<'a> {
    document: &'a Document,
    ancestors: Vec<ObjectId>,
    work: usize,
}

impl Reader<'_> {
    fn read(&mut self, value: &Value, inputs: usize, outputs: usize) -> ReadResult<ReadFunction> {
        let limits = self.document.limits();
        if self.work >= limits.value_nodes {
            return Err(Error::limit("function graph budget exceeded").into());
        }
        self.work += 1;
        if self.ancestors.len() >= limits.depth {
            return Err(Error::limit("function nesting exceeded").into());
        }
        let object = value.as_reference();
        if let Some(id) = object {
            if self.ancestors.contains(&id) {
                return Err(Error::cycle("recursive PDF function").in_object(id).into());
            }
            self.ancestors.push(id);
        }
        let result = self.dictionary(value, inputs, outputs);
        if object.is_some() {
            self.ancestors.pop();
        }
        result.map_err(|e| e.in_object_opt(object))
    }

    fn dictionary(
        &mut self,
        value: &Value,
        inputs: usize,
        outputs: usize,
    ) -> ReadResult<ReadFunction> {
        let document = self.document;
        let dictionary = document.resolve_dictionary(value)?;
        let domain = pairs(document, dictionary, b"Domain", None)?;
        if domain.len() != inputs || inputs == 0 || outputs == 0 {
            return Err(Error::syntax("wrong function input dimension").into());
        }
        let range = if dictionary.contains_key(b"Range") {
            Some(pairs(document, dictionary, b"Range", None)?)
        } else {
            None
        };
        if range.as_ref().is_some_and(|v| v.len() != outputs) {
            return Err(Error::syntax("wrong function output dimension").into());
        }
        let kind = entry(document, dictionary, b"FunctionType")?
            .and_then(Value::as_integer)
            .ok_or(Error::syntax("function has no FunctionType"))?;
        let function = match kind {
            0 => {
                let range = range.ok_or(Error::syntax("sampled function requires Range"))?;
                sampled(document, value, dictionary, domain.clone(), range)?
            }
            2 => exponential(document, dictionary, outputs, &domain, range)?,
            3 => self.stitching(dictionary, outputs, &domain, range)?,
            4 => calculator(document, value, domain.clone(), range)?,
            _ => return Err(Error::syntax("invalid FunctionType").into()),
        };
        Ok(ReadFunction {
            function: Arc::new(function),
            domain,
        })
    }

    fn stitching(
        &mut self,
        dictionary: &Dictionary,
        outputs: usize,
        domain: &[[f64; 2]],
        range: Option<Vec<[f64; 2]>>,
    ) -> ReadResult<Function> {
        let document = self.document;
        let &[[min, max]] = domain else {
            return Err(Error::syntax("stitching function requires one input").into());
        };
        let values = entry(document, dictionary, b"Functions")?
            .and_then(Value::as_array)
            .filter(|v| !v.is_empty())
            .ok_or(Error::syntax("stitching function requires Functions"))?;
        if values.len() > document.limits().value_nodes {
            return Err(Error::limit("stitching function count exceeded").into());
        }
        let bounds = numbers(document, dictionary, b"Bounds", None)?;
        let encode = pairs_unordered(document, dictionary, b"Encode", None)?;
        if bounds.len() + 1 != values.len() || encode.len() != values.len() {
            return Err(Error::syntax("wrong stitching bounds or encode count").into());
        }
        if (values.len() > 1 && min == max)
            || bounds.iter().any(|&v| v < min || v > max)
            || bounds.windows(2).any(|v| v[0] >= v[1])
        {
            return Err(Error::syntax("stitching bounds outside domain or unordered").into());
        }
        let mut functions = Vec::new();
        functions
            .try_reserve_exact(values.len())
            .map_err(|_| Error::limit("stitching function allocation failed"))?;
        for value in values {
            if self.ancestors.len() >= document.limits().depth {
                return Err(Error::limit("function nesting exceeded").into());
            }
            // Direct dictionaries need a depth entry too; object zero is
            // never a valid indirect identity and is not used for cycles.
            self.ancestors.push(ObjectId::new(0, 0));
            let function = self.read(value, 1, outputs);
            self.ancestors.pop();
            functions.push(function?.function);
        }
        Function::stitching(StitchingFunction {
            domain: [min, max],
            range,
            functions,
            bounds,
            encode,
        })
    }
}

fn sampled(
    document: &Document,
    value: &Value,
    d: &Dictionary,
    domain: Vec<[f64; 2]>,
    range: Vec<[f64; 2]>,
) -> ReadResult<Function> {
    let id = value
        .as_reference()
        .ok_or(Error::syntax("sampled function must be a stream reference"))?;
    let object = document.object(id)?;
    if !object.is_stream() {
        return Err(Error::syntax("sampled function must be a stream").into());
    }
    let size = numbers(document, d, b"Size", None)?
        .into_iter()
        .map(|v| {
            if v < 1.0 || v.fract() != 0.0 || v > f64::from(u32::MAX) {
                return Err(Error::syntax("invalid sampled function size").into());
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "positive integral size bounded to u32 above"
            )]
            Ok(v as u32)
        })
        .collect::<ReadResult<Vec<_>>>()?;
    if size.len() != domain.len() {
        return Err(Error::syntax("wrong sampled function size dimension").into());
    }
    let bits = entry(document, d, b"BitsPerSample")?
        .and_then(Value::as_integer)
        .and_then(|v| u8::try_from(v).ok())
        .filter(|v| matches!(v, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32))
        .ok_or(Error::syntax("invalid function BitsPerSample"))?;
    let order = match entry(document, d, b"Order")?.map(Value::as_integer) {
        None | Some(Some(1)) => 1,
        Some(Some(3)) => 3,
        _ => return Err(Error::syntax("invalid sampled interpolation order").into()),
    };
    let mut count = range.len();
    for &size in &size {
        count = count
            .checked_mul(pagura_core::convert::u32_to_usize(size))
            .ok_or(Error::limit("sampled grid size overflows"))?;
    }
    let required = count
        .checked_mul(usize::from(bits))
        .and_then(|v| v.checked_add(7))
        .map(|v| v / 8)
        .filter(|&v| v <= document.limits().decoded_bytes)
        .ok_or(Error::limit("sampled function byte budget exceeded"))?;
    let data = document.stream_data(object)?;
    let samples: Arc<[u8]> = data
        .get(..required)
        .ok_or(Error::syntax("truncated sampled function"))?
        .into();
    let defaults: Vec<_> = size.iter().map(|&n| [0.0, f64::from(n - 1)]).collect();
    let encode = pairs_unordered(document, d, b"Encode", Some(&defaults))?;
    let decode = pairs_unordered(document, d, b"Decode", Some(&range))?;
    if encode.len() != domain.len() || decode.len() != range.len() {
        return Err(Error::syntax("wrong sampled encode or decode dimension").into());
    }
    Function::sampled(
        SampledFunction {
            domain,
            range,
            size,
            bits_per_sample: bits,
            order,
            encode: Some(encode),
            decode: Some(decode),
            samples,
        },
        &document.limits(),
    )
}

fn exponential(
    document: &Document,
    dictionary: &Dictionary,
    outputs: usize,
    domain: &[[f64; 2]],
    range: Option<Vec<[f64; 2]>>,
) -> ReadResult<Function> {
    let &[[min, max]] = domain else {
        return Err(Error::syntax("exponential function requires one input").into());
    };
    let c0 = numbers(document, dictionary, b"C0", Some(&[0.0]))?;
    let c1 = numbers(document, dictionary, b"C1", Some(&[1.0]))?;
    let exponent = entry(document, dictionary, b"N")?
        .and_then(Value::as_number)
        .ok_or(Error::syntax("exponential function has no N"))?;
    if (exponent.fract() != 0.0 && min < 0.0) || (exponent < 0.0 && min <= 0.0 && max >= 0.0) {
        return Err(Error::syntax("exponential domain includes undefined inputs").into());
    }
    if c0.len() != outputs || c1.len() != outputs {
        return Err(Error::syntax("wrong exponential function output dimension").into());
    }
    Function::exponential(ExponentialFunction {
        domain: [min, max],
        range,
        c0,
        c1,
        exponent,
    })
}

fn calculator(
    document: &Document,
    value: &Value,
    domain: Vec<[f64; 2]>,
    range: Option<Vec<[f64; 2]>>,
) -> ReadResult<Function> {
    let range = range.ok_or(Error::syntax("calculator function requires Range"))?;
    let id = value.as_reference().ok_or(Error::syntax(
        "calculator function must be a stream reference",
    ))?;
    let object = document.object(id)?;
    if !object.is_stream() {
        return Err(Error::syntax("calculator function must be a stream").into());
    }
    let program: Arc<[u8]> = document.stream_data(object)?.as_ref().into();
    Function::calculator(
        CalculatorFunction {
            domain,
            range,
            program,
        },
        &document.limits(),
    )
    .map_err(|error| {
        error.within(Position::Stream {
            object: id,
            offset: 0,
        })
    })
}

/// A list of numbers, or `default` when the key is absent.
fn numbers(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    default: Option<&[f64]>,
) -> ReadResult<Vec<f64>> {
    let Some(value) = dictionary.get(key) else {
        return default
            .map(<[f64]>::to_vec)
            .ok_or(Error::syntax("missing function parameter"));
    };
    list(document, value, f64::MAX)
}

/// Pairs of numbers in any order.
fn pairs_unordered(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    default: Option<&[[f64; 2]]>,
) -> ReadResult<Vec<[f64; 2]>> {
    if !dictionary.contains_key(key) {
        return default
            .map(<[[f64; 2]]>::to_vec)
            .ok_or(Error::syntax("missing function parameter"));
    }
    let values = numbers(document, dictionary, key, None)?;
    let (pairs, remainder) = values.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(Error::syntax("function pairs require even length").into());
    }
    Ok(pairs.to_vec())
}

/// Ordered intervals.
fn pairs(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
    default: Option<&[[f64; 2]]>,
) -> ReadResult<Vec<[f64; 2]>> {
    let values = pairs_unordered(document, dictionary, key, default)?;
    if values.iter().any(|r| r[0] > r[1]) {
        return Err(Error::syntax("reversed function range").into());
    }
    Ok(values)
}
