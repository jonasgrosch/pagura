//! Retained ownership, separate from function evaluation and construction.

use super::{Function, Kind};

impl Function {
    /// Bytes retained by this value and its owned data, including `Self`.
    /// Shared child functions are counted per reference, with Arc counters;
    /// this is a conservative ownership estimate, not allocator telemetry.
    pub fn retained_bytes(&self) -> usize {
        self.retained
    }

    /// Each direct child already owns its summary. Construction is linear in
    /// the local payload, even for a deeply shared public stitching DAG.
    pub(super) fn with_retained(mut self) -> Self {
        let body = match &self.kind {
            Kind::Sampled(sampled) => sampled.retained_bytes(),
            Kind::Exponential { c0, delta, .. } => {
                size_of_val(c0.as_ref()).saturating_add(size_of_val(delta.as_ref()))
            }
            Kind::Stitching(stitching) => [
                size_of_val(stitching.functions.as_ref()),
                size_of_val(stitching.bounds.as_ref()),
                size_of_val(stitching.encode.as_ref()),
                stitching
                    .functions
                    .iter()
                    .map(|f| (2 * size_of::<usize>()).saturating_add(f.retained_bytes()))
                    .fold(0, usize::saturating_add),
            ]
            .into_iter()
            .fold(0, usize::saturating_add),
            Kind::Calculator(program) => program.heap_bytes(),
        };
        self.retained = [
            size_of::<Self>(),
            size_of_val(self.domain.as_ref()),
            self.range.as_deref().map_or(0, size_of_val),
            body,
        ]
        .into_iter()
        .fold(0, usize::saturating_add);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExponentialFunction, SampledFunction, StitchingFunction};
    use pagura_core::Limits;
    use std::sync::Arc;

    fn exponential() -> Function {
        Function::exponential(ExponentialFunction {
            domain: [0.0, 1.0],
            range: None,
            c0: vec![0.0],
            c1: vec![1.0],
            exponent: 1.0,
        })
        .unwrap()
    }

    #[test]
    fn function_ownership_includes_boxed_payloads_and_shared_child_references() {
        let leaf = Arc::new(exponential());
        assert_eq!(
            leaf.retained_bytes(),
            size_of::<Function>() + 4 * size_of::<f64>()
        );
        let stitched = Function::stitching(StitchingFunction {
            domain: [0.0, 1.0],
            range: None,
            functions: vec![Arc::clone(&leaf), Arc::clone(&leaf)],
            bounds: vec![0.5],
            encode: vec![[0.0, 1.0]; 2],
        })
        .unwrap();
        let expected = size_of::<Function>()
            + 2 * size_of::<f64>()
            + 2 * size_of::<Arc<Function>>()
            + size_of::<f64>()
            + 4 * size_of::<f64>()
            + 2 * (2 * size_of::<usize>() + leaf.retained_bytes());
        assert_eq!(stitched.retained_bytes(), expected);
        assert_eq!(leaf.retained_bytes(), exponential().retained_bytes());
    }

    #[test]
    fn sampled_table_payload_is_not_just_the_inline_function() {
        let sample = |count| {
            Function::sampled(
                SampledFunction {
                    domain: vec![[0.0, 1.0]],
                    range: vec![[0.0, 1.0]],
                    size: vec![count],
                    bits_per_sample: 8,
                    order: 1,
                    encode: None,
                    decode: None,
                    samples: vec![0; pagura_core::convert::u32_to_usize(count)].into(),
                },
                &Limits::default(),
            )
            .unwrap()
        };
        assert_eq!(
            sample(4096).retained_bytes() - sample(16).retained_bytes(),
            4080
        );
    }

    #[test]
    fn shared_stitching_dags_have_constant_time_saturating_summaries() {
        let mut f = Arc::new(exponential());
        for _ in 0..100 {
            f = Arc::new(
                Function::stitching(StitchingFunction {
                    domain: [0.0, 1.0],
                    range: None,
                    functions: vec![Arc::clone(&f), Arc::clone(&f)],
                    bounds: vec![0.5],
                    encode: vec![[0.0, 1.0]; 2],
                })
                .unwrap(),
            );
        }
        assert_eq!(f.retained_bytes(), usize::MAX);
        assert_eq!(f.as_ref().clone().retained_bytes(), usize::MAX);
    }
}
