//! Navigation stays inert across the host boundary (12.3–12.6).
use super::{Response, Writer};
use pagura::{
    Document, Error, Result,
    navigation::{ActionKind, Destination, DestinationView, NamedAction, Target},
};

fn destination(out: &mut Writer, dest: Destination) -> Result<()> {
    out.count(dest.page)?;
    let (fit, values): (u8, [Option<f64>; 4]) = match dest.view {
        DestinationView::Xyz { left, top, zoom } => (0, [left, top, zoom, None]),
        DestinationView::Fit => (1, [None; 4]),
        DestinationView::FitH(top) => (2, [top, None, None, None]),
        DestinationView::FitV(left) => (3, [left, None, None, None]),
        DestinationView::FitR(r) => (4, [Some(r.x0), Some(r.y0), Some(r.x1), Some(r.y1)]),
        DestinationView::FitB => (5, [None; 4]),
        DestinationView::FitBh(top) => (6, [top, None, None, None]),
        DestinationView::FitBv(left) => (7, [left, None, None, None]),
    };
    out.u8(fit)?;
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
fn target(out: &mut Writer, target: Option<&Target<'_>>) -> Result<()> {
    match target {
        None => out.count(0),
        Some(Target::Destination(dest)) => {
            out.count(1)?;
            out.count(0)?;
            out.u8(1)?;
            destination(out, *dest)
        }
        Some(Target::Actions(sequence)) => {
            out.count(sequence.actions.len())?;
            for action in &sequence.actions {
                out.count(action.parent.map_or(0, |n| n.saturating_add(1)))?;
                match &action.kind {
                    ActionKind::GoTo(dest) => {
                        out.u8(1)?;
                        destination(out, *dest)?;
                    }
                    ActionKind::Uri { uri, base, is_map } => {
                        out.u8(2)?;
                        out.text(utf8(uri)?)?;
                        out.u8(u8::from(base.is_some()))?;
                        if let Some(base) = base {
                            out.text(utf8(base)?)?;
                        }
                        out.u8(u8::from(*is_map))?;
                    }
                    ActionKind::Named(name) => {
                        out.u8(3)?;
                        match name {
                            NamedAction::NextPage => out.text("NextPage")?,
                            NamedAction::PrevPage => out.text("PrevPage")?,
                            NamedAction::FirstPage => out.text("FirstPage")?,
                            NamedAction::LastPage => out.text("LastPage")?,
                            NamedAction::Other(name) => out.text(&name.to_string())?,
                        }
                    }
                    ActionKind::GoToR { .. } => out.u8(4)?,
                    ActionKind::Launch { .. } => out.u8(5)?,
                    ActionKind::JavaScript(_) => out.u8(6)?,
                    ActionKind::Other(name) => {
                        out.u8(7)?;
                        out.text(&name.to_string())?;
                    }
                }
            }
            Ok(())
        }
    }
}
pub(crate) fn links(doc: &Document, page: u32, cap: usize) -> Result<Response> {
    let page = doc.page(
        usize::try_from(page)
            .map_err(|_| Error::invalid_argument("page index exceeds address space"))?,
    )?;
    let links = page.links(doc)?;
    let mut out = Writer::new(cap);
    out.count(links.len())?;
    for link in links {
        if link.quad_points.is_empty() {
            let r = link.rectangle;
            out.count(1)?;
            out.quad(&[r.x0, r.y0, r.x1, r.y0, r.x1, r.y1, r.x0, r.y1])?;
        } else {
            out.count(link.quad_points.len())?;
            for quad in &link.quad_points {
                out.quad(quad)?;
            }
        }
        target(&mut out, link.target.as_ref())?;
    }
    Ok(Response::new(140, out))
}
pub(crate) fn outlines(doc: &Document, cap: usize) -> Result<Response> {
    let items = doc.outlines()?;
    let mut out = Writer::new(cap);
    out.count(items.len())?;
    for item in items {
        out.count(item.depth)?;
        out.text(&item.title)?;
        out.u8(u8::from(item.open))?;
        out.u8(u8::from(item.italic) | (u8::from(item.bold) << 1))?;
        for value in item.color {
            out.f64(value)?;
        }
        target(&mut out, item.target.as_ref())?;
    }
    Ok(Response::new(141, out))
}
pub(crate) fn labels(doc: &Document, cap: usize) -> Result<Response> {
    let labels = doc.page_labels()?;
    let mut out = Writer::new(cap);
    out.count(doc.page_count()?)?;
    for label in labels.iter() {
        out.text(&label?)?;
    }
    Ok(Response::new(142, out))
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
