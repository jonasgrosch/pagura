//! Font budgets, derived once from `Limits` (ADR 0016, ADR 0023).
//!
//! A parse and each outline charge their own meters. Exceeding a `Limits`
//! budget is a `Limit` error; exceeding a ceiling that a font format defines
//! (operand stacks, subroutine nesting, stem counts) is a `Syntax` error,
//! because no budget can admit such a program.
use pagura_core::{Diagnostic, Diagnostics, Error, Limits, Meter, Position, Repair, Result};

/// The `Limits` fields that bound one font program, each used only for its
/// documented meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FontBudget {
    /// `font_work`: work units of one parse or one outline (tokens,
    /// operators, validated table entries, outline points).
    work: usize,
    /// `font_entries`: entries one program holds (glyphs, subroutines,
    /// names, cmap and encoding records), and the points of one outline.
    entries: usize,
    /// `file_bytes` and `decoded_bytes`: the program bytes, which are a
    /// decoded stream or a standalone file.
    input: usize,
    /// `decoded_bytes`: each decoded copy made from the program (Type 1
    /// framing, hexadecimal and cipher decoding).
    decoded: usize,
    /// `string_bytes`: one glyph name or token.
    strings: usize,
    /// `depth`: nested subroutine calls and composite glyphs.
    depth: usize,
    /// `path_segments`: segments of one outline.
    segments: usize,
}

impl FontBudget {
    pub(crate) const fn new(limits: &Limits) -> Self {
        Self {
            work: limits.font_work,
            entries: limits.font_entries,
            input: if limits.file_bytes < limits.decoded_bytes {
                limits.file_bytes
            } else {
                limits.decoded_bytes
            },
            decoded: limits.decoded_bytes,
            strings: limits.string_bytes,
            depth: limits.depth,
            segments: limits.path_segments,
        }
    }

    /// Checks the size of the program bytes.
    pub(crate) fn input(&self, len: usize) -> Result<()> {
        if len > self.input {
            return Err(Error::limit("font program exceeds the byte budget"));
        }
        Ok(())
    }
}

/// The meters and repair sink of one parse or one outline.
pub(crate) struct Work<'a> {
    budget: FontBudget,
    units: Meter,
    entries: Meter,
    segments: Meter,
    diagnostics: &'a mut Diagnostics,
}

impl<'a> Work<'a> {
    pub(crate) const fn new(budget: FontBudget, diagnostics: &'a mut Diagnostics) -> Self {
        Self {
            budget,
            units: Meter::new(budget.work, "font work budget exceeded"),
            entries: Meter::new(budget.entries, "font entry budget exceeded"),
            segments: Meter::new(budget.segments, "glyph outline segment budget exceeded"),
            diagnostics,
        }
    }

    /// Charges `units` of work at buffer offset `at`.
    pub(crate) fn work(&mut self, units: usize, at: Position) -> Result<()> {
        self.units.charge(units).map_err(|e| e.at(at))
    }

    /// Charges `count` held entries (or outline points) at `at`.
    pub(crate) fn entries(&mut self, count: usize, at: Position) -> Result<()> {
        self.entries.charge(count).map_err(|e| e.at(at))
    }

    /// Charges `count` outline segments at `at`.
    pub(crate) fn segments(&mut self, count: usize, at: Position) -> Result<()> {
        self.segments.charge(count).map_err(|e| e.at(at))
    }

    /// Checks one decoded copy of `len` bytes before it is made.
    pub(crate) fn decoded(&self, len: usize, at: Position) -> Result<()> {
        if len > self.budget.decoded {
            return Err(Error::limit("decoded font data budget exceeded").at(at));
        }
        Ok(())
    }

    /// Checks one name or token of `len` bytes.
    pub(crate) fn string(&self, len: usize, at: Position) -> Result<()> {
        if len > self.budget.strings {
            return Err(Error::limit("font name budget exceeded").at(at));
        }
        Ok(())
    }

    /// Checks a nesting `depth` against a format ceiling and the budget.
    pub(crate) fn depth(&self, depth: usize, ceiling: usize, at: Position) -> Result<()> {
        if depth > ceiling {
            return Err(Error::syntax("font nesting exceeds the format limit").at(at));
        }
        if depth > self.budget.depth {
            return Err(Error::limit("font nesting depth budget exceeded").at(at));
        }
        Ok(())
    }

    /// Reports a repair once per kind in this operation (ADR 0003); strict
    /// mode rejects it.
    pub(crate) fn repair(
        &mut self,
        tag: Repair,
        message: &'static str,
        at: Position,
    ) -> Result<()> {
        self.diagnostics
            .repair_once(Diagnostic::new(tag, message).at(at))
    }
}

#[cfg(test)]
mod tests {
    use super::{FontBudget, Work};
    use pagura_core::{Diagnostics, ErrorKind, Limits, Position, Recovery};

    #[test]
    fn budgets_are_limits_and_format_ceilings_are_syntax() {
        let limits = Limits {
            font_work: 3,
            font_entries: 2,
            depth: 4,
            ..Limits::default()
        };
        let mut diagnostics = Diagnostics::new(Recovery::Repair);
        let mut work = Work::new(FontBudget::new(&limits), &mut diagnostics);
        work.work(3, Position::Unknown).unwrap();
        let error = work.work(1, Position::Buffer(7)).unwrap_err();
        assert_eq!(
            (error.kind, error.position),
            (ErrorKind::Limit, Position::Buffer(7))
        );
        work.entries(2, Position::Unknown).unwrap();
        assert_eq!(
            work.entries(1, Position::Unknown).unwrap_err().kind,
            ErrorKind::Limit
        );
        work.depth(4, 10, Position::Unknown).unwrap();
        assert_eq!(
            work.depth(5, 10, Position::Unknown).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(
            work.depth(11, 10, Position::Unknown).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }

    #[test]
    fn input_is_bounded_by_both_file_and_decoded_bytes() {
        for limits in [
            Limits {
                file_bytes: 4,
                ..Limits::default()
            },
            Limits {
                decoded_bytes: 4,
                ..Limits::default()
            },
        ] {
            let budget = FontBudget::new(&limits);
            budget.input(4).unwrap();
            assert_eq!(budget.input(5).unwrap_err().kind, ErrorKind::Limit);
        }
    }
}
