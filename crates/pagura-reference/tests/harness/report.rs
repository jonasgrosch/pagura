//! The harness report: one line per case on standard output (`cargo test
//! -- --nocapture`), and a JSON summary per test written into the directory
//! named by `PAGURA_REFERENCE_REPORT`, when it is set.

use crate::compare::Comparison;
use crate::goldens::Changes;
use pagura_core::Error;
use std::fmt::Write as _;

/// The environment variable naming the report directory.
const REPORT: &str = "PAGURA_REFERENCE_REPORT";

/// What became of one case.
#[derive(Debug, Clone)]
pub enum Status {
    /// A dependency still returns its placeholder error.
    Pending(String),
    /// Something other than a placeholder failed; the case has no result.
    Declined(String),
    /// Compared with the reference.
    Compared(Comparison),
    /// Checked without a comparison (partition invariance).
    Checked,
    /// Changed against the previous renderer's golden.
    Changed(Changes),
    /// A property the case must have does not hold.
    Failed(String),
}

/// One case's line of the report.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The case id, with a variant suffix where a test runs several.
    pub id: String,
    /// Its result.
    pub status: Status,
}

/// The report of one test.
#[derive(Debug, Default)]
pub struct Report {
    entries: Vec<Entry>,
}

impl Report {
    /// Adds a case's result and prints it.
    pub fn push(&mut self, id: impl Into<String>, status: Status) {
        let entry = Entry {
            id: id.into(),
            status,
        };
        println!("{}", line(&entry));
        self.entries.push(entry);
    }

    /// The entries that failed: compared with pixels beyond one level, or
    /// failing a property.
    pub fn failures(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|e| match &e.status {
                Status::Compared(c) => c.beyond > 0,
                Status::Changed(c) => c.beyond > 0,
                Status::Failed(_) => true,
                _ => false,
            })
            .collect()
    }

    /// Prints the totals and writes `<section>.json` into the report
    /// directory, if one is named.
    pub fn finish(&self, section: &str) {
        let mut total = Comparison::default();
        let (mut pending, mut declined, mut compared, mut checked, mut failed) = (0, 0, 0, 0, 0);
        for entry in &self.entries {
            match &entry.status {
                Status::Pending(_) => pending += 1,
                Status::Declined(_) => declined += 1,
                Status::Compared(c) => {
                    compared += 1;
                    total.add(c);
                }
                Status::Checked | Status::Changed(_) => checked += 1,
                Status::Failed(_) => failed += 1,
            }
        }
        println!(
            "{section}: {} cases, {compared} compared, {checked} checked, {pending} pending, {declined} declined, \
             {failed} failed; {} pixels, {} beyond one level, {} changed, exact rounding {:.6}",
            self.entries.len(),
            total.pixels,
            total.beyond,
            total.changed,
            total.match_rate(),
        );
        let Some(directory) = std::env::var_os(REPORT) else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        let mut json = String::new();
        let _ = write!(
            json,
            "{{\"section\":{},\"cases\":{},\"compared\":{compared},\"checked\":{checked},\"pending\":{pending},\
             \"declined\":{declined},\"failed\":{failed},\"pixels\":{},\"beyond\":{},\"changed\":{},\
             \"exact_rounding\":{},\"max_error\":{},\"entries\":[",
            string(section),
            self.entries.len(),
            total.pixels,
            total.beyond,
            total.changed,
            number(total.match_rate()),
            number(total.max_error),
        );
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&entry_json(entry));
        }
        json.push_str("]}\n");
        let written = std::fs::create_dir_all(&directory)
            .and_then(|()| std::fs::write(directory.join(format!("{section}.json")), json));
        if let Err(error) = written {
            println!("{section}: report not written: {error}");
        }
    }
}

fn line(entry: &Entry) -> String {
    match &entry.status {
        Status::Pending(why) => format!("pending   {} ({why})", entry.id),
        Status::Declined(why) => format!("declined  {} ({why})", entry.id),
        Status::Checked => format!("checked   {}", entry.id),
        Status::Failed(why) => format!("FAILED    {} ({why})", entry.id),
        Status::Changed(c) => format!(
            "{} {} {} channels changed against the golden, {} beyond one level; correctly rounded before {}, after {}",
            if c.beyond > 0 {
                "BEYOND   "
            } else {
                "golden   "
            },
            entry.id,
            c.changed.len(),
            c.beyond,
            c.golden_matching,
            c.new_matching,
        ),
        Status::Compared(c) => {
            let beyond: Vec<_> = c.changes.iter().filter(|c| c.beyond()).take(4).collect();
            format!(
                "{} {} pixels {}, beyond {}, changed {}, exact rounding {:.4}, max error {:.3}{}",
                if c.beyond > 0 {
                    "BEYOND   "
                } else {
                    "compared "
                },
                entry.id,
                c.pixels,
                c.beyond,
                c.changed,
                c.match_rate(),
                c.max_error,
                if beyond.is_empty() {
                    String::new()
                } else {
                    format!(" first {beyond:?}")
                },
            )
        }
    }
}

fn entry_json(entry: &Entry) -> String {
    let id = string(&entry.id);
    match &entry.status {
        Status::Pending(why) => format!(
            "{{\"id\":{id},\"status\":\"pending\",\"reason\":{}}}",
            string(why)
        ),
        Status::Declined(why) => format!(
            "{{\"id\":{id},\"status\":\"declined\",\"reason\":{}}}",
            string(why)
        ),
        Status::Failed(why) => format!(
            "{{\"id\":{id},\"status\":\"failed\",\"reason\":{}}}",
            string(why)
        ),
        Status::Checked => format!("{{\"id\":{id},\"status\":\"checked\"}}"),
        Status::Compared(c) => {
            let changes: Vec<String> = c
                .changes
                .iter()
                .map(|c| {
                    format!(
                        "[{},{},{},{},{}]",
                        c.x,
                        c.y,
                        c.channel,
                        c.byte,
                        number(c.exact)
                    )
                })
                .collect();
            format!(
                "{{\"id\":{id},\"status\":\"compared\",\"pixels\":{},\"beyond\":{},\"changed\":{},\
                 \"exact_rounding\":{},\"max_error\":{},\"changes\":[{}]}}",
                c.pixels,
                c.beyond,
                c.changed,
                number(c.match_rate()),
                number(c.max_error),
                changes.join(","),
            )
        }
        Status::Changed(c) => {
            let changes: Vec<String> = c
                .changed
                .iter()
                .map(|(x, y, channel, old, new, exact)| {
                    format!("[{x},{y},{channel},{old},{new},{}]", number(*exact))
                })
                .collect();
            format!(
                "{{\"id\":{id},\"status\":\"changed\",\"channels\":{},\"beyond\":{},\
                 \"golden_correctly_rounded\":{},\"new_correctly_rounded\":{},\"changes\":[{}]}}",
                c.changed.len(),
                c.beyond,
                c.golden_matching,
                c.new_matching,
                changes.join(","),
            )
        }
    }
}

/// An error as report text: kind, message, object and position.
pub fn describe(error: &Error) -> String {
    format!(
        "{}: {} at {:?} in {:?}",
        error.kind, error.message, error.position, error.object
    )
}

/// A JSON string literal.
fn string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON number; non-finite values become null.
fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "null".to_owned()
    }
}
