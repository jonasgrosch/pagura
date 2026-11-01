//! Quantize only rows the preparer has already geometrically resolved.
use super::Geometry;
use crate::FillRule;
use crate::band::{cells::Cells, row::Sink};
use pagura_core::{Error, Meter, Result};
/// Temporary exceptional row. The caller refunds `reserved_bytes` only after
/// dropping the row, keeping its allocation charged while building a packet.
#[derive(Debug)]
pub struct CorrectionRow {
    values: Vec<(u32, u8)>,
    reserved: usize,
}
impl CorrectionRow {
    /// Exact coverage values, including zero cells replacing original edges.
    pub fn values(&self) -> &[(u32, u8)] {
        &self.values
    }
    /// Allocation charge to refund after dropping this temporary row.
    pub fn reserved_bytes(&self) -> usize {
        self.reserved
    }
}
impl Geometry<'_> {
    /// Number of cells in a resolved row; zero for ordinary GPU-covered rows.
    pub fn correction_count(self, y: u32) -> usize {
        if self.0.resolved.mask(&(y..y + 1)) == 0 {
            0
        } else {
            self.0.bounds.width() as usize
        }
    }
    /// Quantizes an exceptional row through the CPU's common cell kernel.
    /// Ordinary geometry is never rasterized here. Scratch and returned
    /// values share the caller's live byte budget; work is cumulatively metered.
    ///
    /// # Errors
    /// Returns a bounded scratch/output allocation or work refusal.
    pub fn corrections(self, y: u32, bytes: &mut Meter, work: &mut Meter) -> Result<CorrectionRow> {
        let count = self.correction_count(y);
        if count == 0 {
            return Ok(CorrectionRow {
                values: Vec::new(),
                reserved: 0,
            });
        }
        let before = bytes.used();
        let mut cells = Cells::new(count, bytes)?;
        let bounds = self.0.bounds;
        if !cells.open((bounds.x0 as usize, bounds.x1 as usize), y..y + 1) {
            return Err(Error::limit("GPU resolved-row scratch"));
        }
        let mut values = Vec::new();
        bytes.reserve(&mut values, count)?;
        work.charge(count)?;
        for x in bounds.x0..bounds.x1 {
            values.push((x, 0));
        }
        for &line in self.0.resolved.lines(&(y..y + 1)) {
            work.charge(cells.line_measured(line))?;
        }
        work.charge(cells.emit(
            0,
            FillRule::NonZero,
            &mut Row {
                base: bounds.x0 as usize,
                values: &mut values,
            },
        ))?;
        drop(cells);
        let reserved = count * size_of::<(u32, u8)>();
        bytes.refund(bytes.used() - before - reserved);
        Ok(CorrectionRow { values, reserved })
    }
}

struct Row<'a> {
    base: usize,
    values: &'a mut [(u32, u8)],
}
impl Sink for Row<'_> {
    fn run(&mut self, a: usize, b: usize, value: u8) {
        let a = a.saturating_sub(self.base).min(self.values.len());
        let b = b.saturating_sub(self.base).min(self.values.len());
        for cell in &mut self.values[a..b.max(a)] {
            cell.1 = value;
        }
    }
}
