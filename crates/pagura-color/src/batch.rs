//! Complete, interleaved points borrowed by the colour/function line APIs.

/// Matched input/output prefixes: every point has both complete strides.
/// Public entry points decide what a zero stride means before borrowing.
pub(crate) struct Batch<'a> {
    inputs: &'a [f64],
    outputs: &'a mut [f64],
    input_stride: usize,
    output_stride: usize,
}

impl<'a> Batch<'a> {
    pub(crate) fn new(
        (inputs, input_stride): (&'a [f64], usize),
        (outputs, output_stride): (&'a mut [f64], usize),
    ) -> Self {
        let input_stride = input_stride.max(1);
        let output_stride = output_stride.max(1);
        let len = (inputs.len() / input_stride).min(outputs.len() / output_stride);
        Self {
            inputs: &inputs[..len * input_stride],
            outputs: &mut outputs[..len * output_stride],
            input_stride,
            output_stride,
        }
    }

    pub(crate) fn output_stride(&self) -> usize {
        self.output_stride
    }

    pub(crate) fn parts(self) -> (&'a [f64], &'a mut [f64]) {
        (self.inputs, self.outputs)
    }

    pub(crate) fn points(self) -> impl Iterator<Item = (&'a [f64], &'a mut [f64])> {
        self.inputs
            .chunks_exact(self.input_stride)
            .zip(self.outputs.chunks_exact_mut(self.output_stride))
    }

    /// Keeps the caller's fixed scratch width and the validated strides.
    pub(crate) fn chunks(self, points: usize) -> impl Iterator<Item = Self> {
        debug_assert!(points != 0);
        let (input_stride, output_stride) = (self.input_stride, self.output_stride);
        self.inputs
            .chunks(points.saturating_mul(input_stride))
            .zip(
                self.outputs
                    .chunks_mut(points.saturating_mul(output_stride)),
            )
            .map(move |(inputs, outputs)| Self {
                inputs,
                outputs,
                input_stride,
                output_stride,
            })
    }
}
