//! Navigation stays inert across the host boundary (12.3–12.6).

use super::Response;
use crate::protocol::{ActionCode, Field, FitCode, OutlineStyle, ResponseKind};
use crate::wire::Writer;
use pagura::navigation::{ActionKind, Destination, DestinationView, NamedAction, Target};
use pagura::{Document, Error, Result};

/// `u32 page | u8 fit | u8 coordinate_mask | f64 present_coordinates[]`.
fn destination(out: &mut Writer, destination: Destination) -> Result<()> {
    out.count(destination.page)?;
    let (fit, values): (FitCode, [Option<f64>; 4]) = match destination.view {
        DestinationView::Xyz { left, top, zoom } => (FitCode::Xyz, [left, top, zoom, None]),
        DestinationView::Fit => (FitCode::Fit, [None; 4]),
        DestinationView::FitH(top) => (FitCode::FitH, [top, None, None, None]),
        DestinationView::FitV(left) => (FitCode::FitV, [left, None, None, None]),
        DestinationView::FitR(r) => (
            FitCode::FitR,
            [Some(r.x0()), Some(r.y0()), Some(r.x1()), Some(r.y1())],
        ),
        DestinationView::FitB => (FitCode::FitB, [None; 4]),
        DestinationView::FitBh(top) => (FitCode::FitBh, [top, None, None, None]),
        DestinationView::FitBv(left) => (FitCode::FitBv, [left, None, None, None]),
    };
    out.u8(fit.code())?;
    let mask = values.iter().enumerate().fold(0, |mask, (i, value)| {
        mask | (u8::from(value.is_some()) << i)
    });
    out.u8(mask)?;
    for value in values.into_iter().flatten() {
        out.f64(value)?;
    }
    Ok(())
}

fn utf8(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| Error::syntax("navigation URI is not UTF-8"))
}

/// `u32 action_count | Action[action_count]`, where an Action is
/// `u32 parent_plus_one | u8 type` and the type's data.
fn target(out: &mut Writer, target: Option<&Target<'_>>) -> Result<()> {
    match target {
        None => out.count(0),
        Some(Target::Destination(dest)) => {
            out.count(1)?;
            out.count(0)?;
            out.u8(ActionCode::GoTo.code())?;
            destination(out, *dest)
        }
        Some(Target::Actions(sequence)) => {
            out.count(sequence.actions.len())?;
            for action in &sequence.actions {
                out.count(action.parent.map_or(0, |n| n.saturating_add(1)))?;
                match &action.kind {
                    ActionKind::GoTo(dest) => {
                        out.u8(ActionCode::GoTo.code())?;
                        destination(out, *dest)?;
                    }
                    ActionKind::Uri { uri, base, is_map } => {
                        out.u8(ActionCode::Uri.code())?;
                        out.text(utf8(uri)?)?;
                        out.flag(base.is_some())?;
                        if let Some(base) = base {
                            out.text(utf8(base)?)?;
                        }
                        out.flag(*is_map)?;
                    }
                    ActionKind::Named(name) => {
                        out.u8(ActionCode::Named.code())?;
                        match name {
                            NamedAction::NextPage => out.text("NextPage")?,
                            NamedAction::PrevPage => out.text("PrevPage")?,
                            NamedAction::FirstPage => out.text("FirstPage")?,
                            NamedAction::LastPage => out.text("LastPage")?,
                            // An escaped PDF name, never an identifier.
                            NamedAction::Other(name) => out.text(&name.to_string())?,
                        }
                    }
                    ActionKind::GoToR { .. } => out.u8(ActionCode::GoToR.code())?,
                    ActionKind::Launch { .. } => out.u8(ActionCode::Launch.code())?,
                    ActionKind::JavaScript(_) => out.u8(ActionCode::JavaScript.code())?,
                    ActionKind::Other(name) => {
                        out.u8(ActionCode::Other.code())?;
                        out.text(&name.to_string())?;
                    }
                }
            }
            Ok(())
        }
    }
}

/// `u32 count | Link[count]`, where a Link is `u32 quad_count |
/// Quad[quad_count] | Target`.
pub(crate) fn links(document: &Document, page: Field<u32>, cap: usize) -> Result<Response> {
    let links = document
        .page(page.index()?)
        .map_err(|error| error.at(page.position()))?
        .links(document)?;
    let mut out = Writer::new(cap);
    out.count(links.len())?;
    for link in links {
        if link.quad_points.is_empty() {
            // One quad from the annotation rectangle (Table 176).
            let r = link.rectangle;
            out.count(1)?;
            out.f64s(&[
                r.x0(),
                r.y0(),
                r.x1(),
                r.y0(),
                r.x1(),
                r.y1(),
                r.x0(),
                r.y1(),
            ])?;
        } else {
            out.count(link.quad_points.len())?;
            for quad in &link.quad_points {
                out.f64s(quad)?;
            }
        }
        target(&mut out, link.target.as_ref())?;
    }
    Ok(Response::new(ResponseKind::Links, out))
}

/// `u32 count | Outline[count]`, where an Outline is `u32 depth | text title
/// | u8 initially_open | u8 style | f64 red, green, blue | Target`.
pub(crate) fn outlines(document: &Document, cap: usize) -> Result<Response> {
    let items = document.outlines()?;
    let mut out = Writer::new(cap);
    out.count(items.len())?;
    for item in items {
        out.count(item.depth)?;
        out.text(&item.title)?;
        out.flag(item.open)?;
        let mut style = OutlineStyle::default();
        if item.italic {
            style.insert(OutlineStyle::ITALIC);
        }
        if item.bold {
            style.insert(OutlineStyle::BOLD);
        }
        out.u8(style.bits())?;
        out.f64s(&item.color)?;
        target(&mut out, item.target.as_ref())?;
    }
    Ok(Response::new(ResponseKind::Outlines, out))
}

/// `u32 count | text label[count]`, one label per page.
pub(crate) fn labels(document: &Document, cap: usize) -> Result<Response> {
    let labels = document.page_labels()?;
    let mut out = Writer::new(cap);
    out.count(document.page_count()?)?;
    for label in labels.iter() {
        out.text(&label?)?;
    }
    Ok(Response::new(ResponseKind::Labels, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_destination_fit_has_fixed_masks_and_coordinates() {
        let rectangle = pagura::Rect::from_corners(1., 2., 30., 40.).unwrap();
        let cases = [
            (
                DestinationView::Xyz {
                    left: None,
                    top: Some(42.),
                    zoom: None,
                },
                0,
                2,
                vec![42.],
            ),
            (DestinationView::Fit, 1, 0, vec![]),
            (DestinationView::FitH(None), 2, 0, vec![]),
            (DestinationView::FitV(Some(12.)), 3, 1, vec![12.]),
            (
                DestinationView::FitR(rectangle),
                4,
                15,
                vec![1., 2., 30., 40.],
            ),
            (DestinationView::FitB, 5, 0, vec![]),
            (DestinationView::FitBh(Some(7.)), 6, 1, vec![7.]),
            (DestinationView::FitBv(None), 7, 0, vec![]),
        ];
        for (view, fit, mask, coordinates) in cases {
            let mut writer = Writer::new(1024);
            destination(&mut writer, Destination { page: 5, view }).unwrap();
            let mut expected = vec![5, 0, 0, 0, fit, mask];
            for value in coordinates {
                expected.extend(f64::to_le_bytes(value));
            }
            assert_eq!(writer.finish(), expected);
        }
    }
}
