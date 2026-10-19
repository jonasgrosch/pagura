//! Adapters from PDF image dictionaries to the codecs of `pagura-imagecodec`.
//! Each returns the decoded [`Pixels`] of its image.
pub(crate) mod dct;
pub(crate) mod fax;
pub(crate) mod jbig2;
pub(crate) mod jpx;

use crate::{Precision, Samples};

/// Decoded color samples, before the dictionary's painting parameters.
#[derive(Debug)]
pub(crate) struct Pixels {
    pub(crate) components: u8,
    pub(crate) precision: Precision,
    pub(crate) samples: Samples,
}

impl Pixels {
    /// One-bit samples of a single component, as CCITT and JBIG2 produce.
    pub(crate) const fn bilevel(samples: Vec<u8>) -> Self {
        Self {
            components: 1,
            precision: Precision::Uniform(1),
            samples: Samples::Bytes(samples),
        }
    }
}
