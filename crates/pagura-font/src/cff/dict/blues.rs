//! Unhinted blue-zone recovery: TN 5176 section 15/Table 23; Type 1 5.3-5.5.
use super::{Dict, Operator};
use crate::budget::Work;
use crate::repairs;
use pagura_core::Result;

/// The Private DICT operators that hold blue-zone pairs.
const ZONES: [Operator; 4] = [
    Operator::BLUE_VALUES,
    Operator::OTHER_BLUES,
    Operator::FAMILY_BLUES,
    Operator::FAMILY_OTHER_BLUES,
];

impl Dict {
    pub(in crate::cff) fn check_blues(&self, work: &mut Work<'_>) -> Result<()> {
        let mut fault = self.blue_order_fault;
        for op in ZONES {
            let Some(values) = self.values(op) else {
                continue;
            };
            // Each pair's second delta is its upper-minus-lower distance.
            // Zone order is unrestricted; accumulating unused absolute hint
            // coordinates would add work and unnecessary float overflow.
            if values
                .as_chunks::<2>()
                .0
                .iter()
                .any(|&[_, size]| size < 0.0)
            {
                fault.get_or_insert(self.at);
            }
        }
        if let Some(at) = fault {
            work.repair(
                repairs::CFF_BLUE_ORDER,
                "CFF blue hint ordering ignored for unhinted outlines",
                at,
            )?;
        }
        Ok(())
    }
}
