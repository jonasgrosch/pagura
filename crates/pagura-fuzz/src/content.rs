//! `content`: a content stream records into a scene or fails with a
//! structured error (clause 7.8.2, Annex A; ADR 0034), and the scene renders
//! so that a tile equals the same pixels of the whole page.
//!
//! The input is the content stream of a fixed 32 × 32 page whose resources
//! name one of each kind: `/GS` and `/GS1` graphics states; `/C`
//! (Separation), `/N` (DeviceN), `/Palette` (Indexed), `/Lab` and `/ICC`
//! colour spaces; the `/Fm` form and `/Im` image XObjects; the Type 3 font
//! `/F`; the tiling pattern `/P`; the axial shading `/Sh`; and the optional
//! content groups `/On` and `/Off` with the membership dictionary
//! `/Members`.

use super::wrapper::Pdf;
use pagura::{Document, OpenOptions, RenderOptions, Tile};
use std::sync::Arc;

/// Objects 5 onwards, after the catalog (1), page tree (2), page (3) and
/// content stream (4).
const RESOURCES: &[u8] = b"<< /ExtGState << /GS << /LW 2 /LC 1 /LJ 1 /ML 4 /D [[2 1] 0] /SA true >> \
    /GS1 << /CA .5 /ca .5 /BM /Multiply /Font [12 0 R 8] >> >> \
    /ColorSpace << /C [/Separation /Spot /DeviceRGB << /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 0 0] /N 1 >>] \
    /N [/DeviceN [/A /B] /DeviceRGB 9 0 R] /Palette [/Indexed /DeviceRGB 1 <000000ff0000>] \
    /Lab [/Lab << /WhitePoint [.9505 1 1.089] >>] /ICC [/ICCBased 8 0 R] >> \
    /XObject << /Fm 10 0 R /Im 11 0 R >> /Font << /F 12 0 R >> /Pattern << /P 13 0 R >> \
    /Shading << /Sh << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 32 32] \
    /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >> >> \
    /Properties << /On 6 0 R /Off 7 0 R /Members 15 0 R >> >>";

/// The whole page and one interior tile, in device pixels at 72 dpi.
const TILE: Tile = Tile {
    x: 5,
    y: 9,
    width: 17,
    height: 13,
};

/// Records the page once and renders it whole and as one tile.
///
/// # Panics
///
/// When the input violates a checked property: that is a finding.
pub fn run(data: &[u8]) {
    let options = OpenOptions {
        limits: super::limits(),
        ..OpenOptions::default()
    };
    let Some(document) = super::checked(Document::open(wrap(data), &options)) else {
        return;
    };
    let context = pagura::render_context(Arc::new(document));
    let document = context.document();
    let Some(page) = super::checked(document.page(0)) else {
        return;
    };
    if let Some(scene) = super::checked(context.scene(page)) {
        let render = RenderOptions::default();
        let whole = super::checked(context.render_scene(&scene, &render, None));
        let tile = super::checked(context.render_scene(&scene, &render, Some(TILE)));
        if let (Some(whole), Some(tile)) = (whole, tile) {
            assert_eq!((whole.width(), whole.height()), (32, 32), "page size");
            assert_eq!((tile.width(), tile.height()), (TILE.width, TILE.height));
            let row = usize::try_from(TILE.width * 4).unwrap_or(usize::MAX);
            let x = usize::try_from(TILE.x * 4).unwrap_or(usize::MAX);
            for y in 0..TILE.height {
                let expected = whole.row(TILE.y + y).and_then(|r| r.get(x..x + row));
                assert_eq!(tile.row(y), expected, "tile row {y} differs from the page");
            }
        }
    }
    super::assert_diagnostics(&document.diagnostics(), options.recovery);
}

/// The fixed one-page document around `content`, also the page of
/// [`super::extraction`].
pub(crate) fn wrap(content: &[u8]) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.object(b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R 7 0 R] /D << /OFF [7 0 R] >> >> >>");
    pdf.object(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    pdf.object(
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 32 32] /Resources 5 0 R /Contents 4 0 R >>",
    );
    pdf.stream(b"", content);
    pdf.object(RESOURCES);
    pdf.object(b"<< /Type /OCG /Name (On) >>");
    pdf.object(b"<< /Type /OCG /Name (Off) >>");
    pdf.stream(b"/N 3 /Alternate /DeviceRGB", b"not an ICC profile");
    pdf.stream(
        b"/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]",
        b"{ 2 copy add 2 div }",
    );
    pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16] /Resources << >>",
        b"0 0 1 rg 0 0 8 8 re f",
    );
    pdf.stream(
        b"/Type /XObject /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray",
        &[0, 85, 170, 255],
    );
    pdf.object(b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [.001 0 0 .001 0 0] \
        /CharProcs << /a 14 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> \
        /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << >> >>");
    pdf.stream(
        b"/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 4 4] /XStep 4 /YStep 4 /Resources << >>",
        b"0 0 2 2 re f",
    );
    pdf.stream(b"", b"1000 0 d0 0 0 1000 1000 re f");
    pdf.object(b"<< /Type /OCMD /OCGs [6 0 R 7 0 R] /P /AnyOn >>");
    pdf.finish()
}
