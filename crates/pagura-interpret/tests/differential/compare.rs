//! One page interpreted by the old renderer's compiler and by the new
//! interpreter, and the differences between them.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers fail loudly"
)]

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
    /// Differences the new design makes on purpose: Type 4 programs that
    /// cannot be validated statically are rejected when read.
    pub intended: Vec<String>,
    /// Scenes and text that matched.
    pub matched: usize,
}

/// Whether the new side's failure is an intended difference: pagura-color
/// validates Type 4 programs statically and rejects programs whose stack
/// or operand types depend on their data.
fn intended(new: &Error) -> bool {
    new.feature() == Some(pagura_color::features::CALCULATOR)
        || (new.kind == ErrorKind::Syntax && new.message.starts_with("calculator"))
}

/// Whether `error` is the stub error of a package that has not landed.
pub fn pending(error: &Error) -> bool {
    error.kind == ErrorKind::InvalidArgument && error.message.contains("not implemented")
}

fn identity(
    error: &Error,
) -> (
    ErrorKind,
    Option<pagura_core::Repair>,
    Option<pagura_core::ObjectId>,
    pagura_core::Position,
) {
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
pub fn old_glyphs(list: &pagura_legacy::DisplayList) -> usize {
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
    let old = pagura_legacy::compile_page(&old_doc, old_page);
    let new = {
        let mut device = crate::decoding::Decoding::new(&new_doc);
        Interpreter::new(&new_doc)
            .run(new_page, &mut device)
            .and_then(|()| device.finish())
    };
    let old_diagnostics = sorted(old_doc.diagnostics());
    let new_diagnostics = sorted(new_doc.diagnostics());
    let scene = match (&old, &new) {
        (_, Err(error)) if pending(error) => {
            outcome.pending.push(format!("{label}: scene: {error}"));
            Scene::Pending
        }
        (_, Err(error)) if intended(error) => {
            outcome.intended.push(format!("{label}: scene: {error}"));
            Scene::Pending
        }
        (Ok(list), Ok(scene)) => {
            let (marks, glyphs) = counts(scene);
            let expected = (list.len(), old_glyphs(list));
            if (marks, glyphs) == expected && old_diagnostics == new_diagnostics {
                outcome.matched += 1;
            } else if (marks, glyphs) == expected
                && hidden_image_decodes(&new_doc, &old_diagnostics, &new_diagnostics)
            {
                outcome.intended.push(format!(
                    "{label}: the old compiler also decoded images hidden by optional content"
                ));
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
            } else if image_data_first(&new_doc, old, new) {
                outcome.intended.push(format!(
                    "{label}: scene error {new} where the old compiler first decoded the image's data: {old}"
                ));
            } else {
                outcome
                    .mismatches
                    .push(format!("{label}: scene error {new} vs old {old}"));
            }
            Scene::FailedAlike
        }
        (Err(old), Ok(_)) => {
            outcome.mismatches.push(format!(
                "{label}: scene succeeds where old fails with {old}"
            ));
            Scene::Other
        }
        (Ok(_), Err(new)) => {
            outcome.mismatches.push(format!(
                "{label}: scene fails with {new} where old succeeds"
            ));
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

/// Whether the old text failure `old` comes from work the text device does
/// not need: interpreting with every need (images decoded, as the old
/// compiler did) fails the same way.
fn paint_work(bytes: &[u8], index: usize, recovery: Recovery, old: &Error) -> bool {
    let Some(document) = open(bytes, recovery) else {
        return false;
    };
    let Ok(page) = document.page(index) else {
        return false;
    };
    let mut device = crate::decoding::Decoding::with_needs(&document, pagura_interpret::Needs::ALL);
    Interpreter::new(&document)
        .run(page, &mut device)
        .is_err_and(|error| identity(&error) == identity(old))
}

/// Whether the old compiler failed decoding the data of an image while the
/// new interpretation fails at a `Do`, reading an image's description (its
/// colour space or masks): the old compiler decoded an image's data before
/// reading its colour space, and the interpreter leaves the data to
/// painting.
fn image_data_first(document: &Document, old: &Error, new: &Error) -> bool {
    let image = old.object.is_some_and(|id| {
        document
            .object(id)
            .ok()
            .and_then(|o| o.dictionary())
            .is_some_and(|dictionary| dictionary.is_name(b"Subtype", b"Image"))
    });
    let pagura_core::Position::Stream { object, offset } = new.position else {
        return false;
    };
    let at_do = document
        .object(object)
        .ok()
        .and_then(|o| document.stream_data(o).ok())
        .is_some_and(|data| {
            data.get(offset..)
                .is_some_and(|rest| rest.starts_with(b"Do"))
        });
    image && at_do
}

/// Whether the old diagnostics are the new ones plus repairs of decoding
/// image XObjects that the new interpretation never painted (the old
/// compiler decoded images hidden by optional content too).
fn hidden_image_decodes(document: &Document, old: &[Diagnostic], new: &[Diagnostic]) -> bool {
    new.iter().all(|d| old.contains(d))
        && old.iter().filter(|d| !new.contains(d)).all(|d| {
            d.object.is_some_and(|id| {
                document
                    .object(id)
                    .ok()
                    .and_then(|o| o.dictionary())
                    .is_some_and(|dictionary| dictionary.is_name(b"Subtype", b"Image"))
            })
        })
}

/// Diagnostics in a fixed order: the document lists them in discovery
/// order, which follows when objects are first loaded, and the interpreter
/// reads some objects (the Info dictionary for the print intent) earlier
/// than the old compiler did.
fn sorted(diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut diagnostics = diagnostics;
    diagnostics.sort_by_key(|d| format!("{d:?}"));
    diagnostics
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
fn text(
    label: &str,
    bytes: &[u8],
    index: usize,
    recovery: Recovery,
    scene: Scene,
    outcome: &mut Outcome,
) {
    let (Some(old_doc), Some(new_doc)) = (open(bytes, recovery), open(bytes, recovery)) else {
        return;
    };
    let (Ok(old_page), Ok(new_page)) = (old_doc.page(index), new_doc.page(index)) else {
        return;
    };
    let old = pagura_legacy::text_page(&old_doc, old_page, None);
    let new = Interpreter::new(&new_doc).text(new_page).and_then(|text| {
        TextPage::from_positioned(text, LayoutOptions::default(), new_doc.limits())?
            .with_structure(&new_doc, new_page)
    });
    match (&old, &new) {
        (_, Err(error)) if pending(error) => {
            outcome.pending.push(format!("{label}: text: {error}"));
        }
        (Ok(old), Ok(new)) => {
            let (old, new) = (
                without_estimates(&format!("{old:?}")),
                without_estimates(&format!("{new:?}")),
            );
            if old == new {
                outcome.matched += 1;
            } else {
                if let Some(directory) = std::env::var_os("PAGURA_DIFFERENTIAL_DUMP") {
                    let stem = label.replace(['/', ' '], "_");
                    let directory = std::path::PathBuf::from(directory);
                    std::fs::write(
                        directory.join(format!("{stem}.old")),
                        old.replace(", ", ",\n"),
                    )
                    .unwrap();
                    std::fs::write(
                        directory.join(format!("{stem}.new")),
                        new.replace(", ", ",\n"),
                    )
                    .unwrap();
                }
                outcome
                    .mismatches
                    .push(format!("{label}: positioned text differs"));
            }
        }
        (Err(old), Err(new)) => {
            if identity(old) == identity(new) {
                outcome.matched += 1;
            } else if scene == Scene::FailedAlike || paint_work(bytes, index, recovery, old) {
                // The old text failed in painting work the text device
                // skips, and the new one fails later in the content.
                outcome
                    .intended
                    .push(format!("{label}: text error {new} after old {old}"));
            } else {
                outcome
                    .mismatches
                    .push(format!("{label}: text error {new} vs old {old}"));
            }
        }
        (Err(old), Ok(_)) if paint_work(bytes, index, recovery, old) => outcome.matched += 1,
        (Err(old), Ok(_)) => match scene {
            Scene::FailedAlike => outcome.matched += 1,
            Scene::Pending => outcome
                .pending
                .push(format!("{label}: text: old fails with {old}")),
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

/// Compares page `index` of `bytes` drawn with an untouched form session,
/// which honours `NeedAppearances` (12.7.4.3).
pub fn filled_page(name: &str, bytes: &[u8], index: usize, outcome: &mut Outcome) {
    let label = format!("{name} p{index} filled");
    let (Some(old_doc), Some(new_doc)) =
        (open(bytes, Recovery::Repair), open(bytes, Recovery::Repair))
    else {
        return;
    };
    let (Ok(Some(old_filler)), Ok(Some(new_filler))) =
        (old_doc.form_filler(), new_doc.form_filler())
    else {
        return;
    };
    let (Ok(old_page), Ok(new_page)) = (old_doc.page(index), new_doc.page(index)) else {
        return;
    };
    let old = pagura_legacy::compile_filled_page(&old_filler, old_page, None);
    let new = {
        let mut device = crate::decoding::Decoding::new(&new_doc);
        Interpreter::new(&new_doc)
            .with_filler(&new_filler)
            .run(new_page, &mut device)
            .and_then(|()| device.finish())
    };
    match (&old, &new) {
        (_, Err(error)) if pending(error) => {
            outcome.pending.push(format!("{label}: {error}"));
        }
        (Ok(list), Ok(scene)) => {
            let (marks, glyphs) = counts(scene);
            if (marks, glyphs) == (list.len(), old_glyphs(list))
                && sorted(old_doc.diagnostics()) == sorted(new_doc.diagnostics())
            {
                outcome.matched += 1;
            } else {
                outcome.mismatches.push(format!(
                    "{label}: marks/glyphs {:?} vs old {:?}; diagnostics {:?} vs old {:?}",
                    (marks, glyphs),
                    (list.len(), old_glyphs(list)),
                    tags(&new_doc.diagnostics()),
                    tags(&old_doc.diagnostics())
                ));
            }
        }
        (Err(old), Err(new)) if identity(old) == identity(new) => outcome.matched += 1,
        _ => outcome.mismatches.push(format!(
            "{label}: {:?} vs old {:?}",
            new.as_ref().map(|_| ()),
            old.as_ref().map(|_| ())
        )),
    }
}

/// Compares the text of every stored normal appearance of page `index`'s
/// annotations, extracted on its own.
pub fn appearances(name: &str, bytes: &[u8], index: usize, outcome: &mut Outcome) {
    let Some(document) = open(bytes, Recovery::Repair) else {
        return;
    };
    let Ok(page) = document.page(index) else {
        return;
    };
    let Some(annotations) = page
        .dictionary()
        .get(b"Annots")
        .and_then(|v| document.resolve(v).ok())
        .and_then(pagura_core::Value::as_array)
    else {
        return;
    };
    for annotation in annotations {
        let Ok(d) = document.resolve_dictionary(annotation) else {
            continue;
        };
        let Some(id) = d
            .get(b"AP")
            .and_then(|ap| document.resolve_dictionary(ap).ok())
            .and_then(|ap| ap.get(b"N"))
            .and_then(pagura_core::Value::as_reference)
        else {
            continue;
        };
        let Some(rect) = d
            .get(b"Rect")
            .and_then(|r| document.resolve(r).ok())
            .and_then(pagura_core::Value::as_array)
            .and_then(pagura_core::Rect::from_array)
        else {
            continue;
        };
        let label = format!("{name} p{index} appearance {id}");
        let old = pagura_legacy::appearance_text(&document, page, id, rect, None);
        let new = Interpreter::new(&document).appearance_text(page, id, rect);
        match (old, new) {
            (_, Err(error)) if pending(&error) => outcome.pending.push(format!("{label}: {error}")),
            (Ok(old), Ok(new)) if format!("{old:?}") == format!("{new:?}") => outcome.matched += 1,
            (Err(old), Err(new)) if identity(&old) == identity(&new) => outcome.matched += 1,
            (old, new) => outcome.mismatches.push(format!(
                "{label}: {:?} vs old {:?}",
                new.map(|t| t.glyphs.len()),
                old.map(|t| t.glyphs.len())
            )),
        }
    }
}
