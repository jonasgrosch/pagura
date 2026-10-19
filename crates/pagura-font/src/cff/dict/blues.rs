//! Unhinted blue-zone recovery: TN 5176 section 15/Table 23; Type 1 5.3-5.5.
use super::Dict;
use crate::budget::Work;
use crate::repairs;
use pagura_core::{Position, Result};

impl Dict {
    pub(in crate::cff) fn check_blues(&self, work: &mut Work<'_>) -> Result<()> {
        let mut fault = self.blue_order_fault;
        for op in 6..=9 {
            let Some(values) = self.values(op) else {
                continue;
            };
            // Each pair's second delta is its upper-minus-lower distance.
            // Zone order is unrestricted; accumulating unused absolute hint
            // coordinates would add work and unnecessary float overflow.
            if values.as_chunks::<2>().0.iter().any(|pair| pair[1] < 0.0) {
                fault.get_or_insert(self.at);
            }
        }
        if let Some(at) = fault {
            work.repair(
                repairs::CFF_BLUE_ORDER,
                "CFF blue hint ordering ignored for unhinted outlines",
                Position::Buffer(at),
            )?;
        }
        Ok(())
    }
}
