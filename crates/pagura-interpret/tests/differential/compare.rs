//! One page interpreted by the old renderer's compiler and by the new
//! interpreter, and the differences between them.

use pagura_core::{Diagnostic, Error, ErrorKind, Recovery};
use pagura_document::{Document, OpenOptions};
use pagura_extract::{LayoutOptions, TextPage};
use pagura_interpret::{Interpreter, Node};

/// The outcome of comparing one page.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Differences that are defects.
    pub mismatches: Vec<String>,
    /// Comparisons the new side could not make yet: a function, colour
    /// conversion or image source of another package is still a stub.
    pub pending: Vec<String>,
    /// Scenes and text that matched.
    pub matched: usize,
}

/// Whether `error` is the stub error of a package that has not landed.
pub fn pending(error: &Error) -> bool {
    error.kind == ErrorKind::InvalidArgument && error.message.contains("not implemented")
}

fn identity(error: &Error) -> (ErrorKind, Option<pagura_core::Repair>, Option<pagura_core::ObjectId>, pagura_core::Position) {
    (error.kind, error.repair, error.object, error.position)
}

fn open(bytes: &[u8], recovery: Recovery) -> Option<Document> {
    Document::open(
        bytes.to_vec(),
        &OpenOptions {
            recovery,
            ..OpenOptions::default()
        },
    )
    .ok()
}

/// Marks and glyph placements of the page's own tree, as the old display
/// list counts them: soft masks and tiling cells excluded.
pub fn counts(scene: &pagura_interpret::Scene) -> (usize, usize) {
    let mut marks = 0;
    let mut glyphs = 0;
    let mut stack = vec![scene.root];
    while let Some(group) = stack.pop() {
        for &child in &scene.group(group).expect("group").children {
            match scene.node(child).expect("node") {
                Node::Glyphs(run) => {
                    marks += run.placements.len();
                    glyphs += run.placements.len();
                }
                Node::Group(id) => stack.push(*id),
                _ => marks += 1,
            }
        }
    }
    (marks, glyphs)
}

/// Glyph placements of an old display list outside soft masks and pattern
/// cells, read from its debug form (the count is private to the old crate).
pub fn old_glyphs(list: &pagura_render::DisplayList) -> usize {
    let text = format!("{list:?}");
    let bytes = text.as_bytes();
    let skips: [&[u8]; 2] = [b"mask: Some(", b"selected: Some("];
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i = skip_string(bytes, i);
            continue;
        }
        if let Some(skip) = skips.iter().find(|s| bytes[i..].starts_with(s)) {
            i += skip.len();
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'"' => {
                        i = skip_string(bytes, i);
                        continue;
                    }
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"Placement {") {
            count += 1;
        }
        i += 1;
    }
    count
}

fn skip_string(bytes: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// Compares page `index` of `bytes` under `recovery`.
pub fn page(name: &str, bytes: &[u8], index: usize, recovery: Recovery, outcome: &mut Outcome) {
    let label = format!("{name} p{index} {recovery:?}");
    let (Some(old_doc), Some(new_doc)) = (open(bytes, recovery), open(bytes, recovery)) else {
        return;
    };
    let (Ok(old_page), Ok(new_page)) = (old_doc.page(index), new_doc.page(index)) else {
        return;
    };
    let old = pagura_render::compile_page(&old_doc, old_page);
    let new = Interpreter::new(&new_doc).scene(new_page);
    let old_diagnostics: Vec<Diagnostic> = old_doc.diagnostics();
    let new_diagnostics = new_doc.diagnostics();
    let scene = match (&old, &new) {
        (_, Err(error)) if pending(error) => {
            outcome.pending.push(format!("{label}: scene: {error}"));
            Scene::Pending
        }
        (Ok(list), Ok(scene)) => {
            let (marks, glyphs) = counts(scene);
            let expected = (list.len(), old_glyphs(list));
            if (marks, glyphs) == expected && old_diagnostics == new_diagnostics {
                outcome.matched += 1;
            } else {
                outcome.mismatches.push(format!(
                    "{label}: scene marks/glyphs {:?} vs old {expected:?}; diagnostics {:?} vs old {:?}",
                    (marks, glyphs),
                    tags(&new_diagnostics),
                    tags(&old_diagnostics)
                ));
            }
            Scene::Other
        }
        (Err(old), Err(new)) => {
            if identity(old) == identity(new) {
                outcome.matched += 1;
            } else {
                outcome
                    .mismatches
                    .push(format!("{label}: scene error {new} vs old {old}"));
            }
            Scene::FailedAlike
        }
        (Err(old), Ok(_)) => {
            outcome
                .mismatches
                .push(format!("{label}: scene succeeds where old fails with {old}"));
            Scene::Other
        }
        (Ok(_), Err(new)) => {
            outcome
                .mismatches
                .push(format!("{label}: scene fails with {new} where old succeeds"));
            Scene::Other
        }
    };
    text(&label, bytes, index, recovery, scene, outcome);
}

/// A text page's debug form without its retained-byte estimates, which
/// count allocation capacities: decoded `ActualText` and `Alt` strings keep
/// the old interpreter's spare capacity there and are exact here.
fn without_estimates(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("retained: ") {
        let (before, after) = rest.split_at(start);
        out.push_str(before);
        let digits = after["retained: ".len()..]
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len() - "retained: ".len());
        out.push_str("retained: _");
        rest = &after["retained: ".len() + digits..];
    }
    out.push_str(rest);
    out
}

fn tags(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| format!("{} {:?} {:?}", d.tag(), d.object, d.position))
        .collect()
}

/// How the scenes of a page compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scene {
    /// The old and new scenes failed with the same error.
    FailedAlike,
    /// The new scene needs a package that has not landed.
    Pending,
    /// Anything else.
    Other,
}

/// Compares the page's text. Text the new interpreter extracts where the
/// old one failed is expected when the old failure came from painting work,
/// which the text device skips: when both scenes failed alike.
fn text(label: &str, bytes: &[u8], index: usize, recovery: Recovery, scene: Scene, outcome: &mut Outcome) {
    let (Some(old_doc), Some(new_doc)) = (open(bytes, recovery), open(bytes, recovery)) else {
        return;
    };
    let (Ok(old_page), Ok(new_page)) = (old_doc.page(index), new_doc.page(index)) else {
        return;
    };
    let old = pagura_render::text_page(&old_doc, old_page, None);
    let new = Interpreter::new(&new_doc).text(new_page).and_then(|text| {
        TextPage::from_positioned(text, LayoutOptions::default(), new_doc.limits())?
            .with_structure(&new_doc, new_page)
    });
    match (&old, &new) {
        (_, Err(error)) if pending(error) => {
            outcome.pending.push(format!("{label}: text: {error}"));
        }
        (Ok(old), Ok(new)) => {
            let (old, new) = (without_estimates(&format!("{old:?}")), without_estimates(&format!("{new:?}")));
            if old == new {
                outcome.matched += 1;
            } else {
                if let Some(directory) = std::env::var_os("PAGURA_DIFFERENTIAL_DUMP") {
                    let stem = label.replace(['/', ' '], "_");
                    let directory = std::path::PathBuf::from(directory);
                    std::fs::write(directory.join(format!("{stem}.old")), old.replace(", ", ",\n")).unwrap();
                    std::fs::write(directory.join(format!("{stem}.new")), new.replace(", ", ",\n")).unwrap();
                }
                outcome
                    .mismatches
                    .push(format!("{label}: positioned text differs"));
            }
        }
        (Err(old), Err(new)) => {
            if identity(old) == identity(new) {
                outcome.matched += 1;
            } else {
                outcome
                    .mismatches
                    .push(format!("{label}: text error {new} vs old {old}"));
            }
        }
        (Err(old), Ok(_)) => match scene {
            Scene::FailedAlike => outcome.matched += 1,
            Scene::Pending => outcome.pending.push(format!("{label}: text: old fails with {old}")),
            Scene::Other => outcome.mismatches.push(format!(
                "{label}: text succeeds where old fails with {old}, not from painting"
            )),
        },
        (Ok(_), Err(new)) => {
            outcome
                .mismatches
                .push(format!("{label}: text fails with {new} where old succeeds"));
        }
    }
}
