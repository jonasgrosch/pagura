//! Optional semantic output from the existing interpreter (9.4.4; 14.6).
use super::{
    Arc, Compiler, Dictionary, Error, Glyph, GlyphShape, Rc, RefCell, Result, Transform, Value,
    cache, resource,
};
use pagura_core::{Name, ObjectId, Rect};
use pagura_raster::Point;
use pagura_text::{
    LayoutOptions, MarkedContent, MarkedSpan, PositionedGlyph, PositionedText, Quad, TextFont,
    TextPage,
};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(in crate::interpreter) struct Extraction {
    output: Rc<RefCell<Output>>,
    path: Arc<[Arc<MarkedContent>]>,
    spans: Vec<usize>,
}
#[derive(Default)]
struct Output {
    text: PositionedText,
    fonts: BTreeMap<usize, Font>,
    bytes: usize,
    show: usize,
}
struct Font {
    index: usize,
    bbox: Option<Rect>,
}

/// Compiles a page and extracts its positioned text through the same
/// interpretation, returning the text separately from the display list. `None`
/// keeps explicit failures for fonts that need a provider. Coordinates are
/// default user space, before page rotation, UserUnit and device scaling.
/// Annotation appearances, pattern cells, soft masks and Type 3 procedure
/// internals are paint, not page text. Use [`crate::RenderContext`] to share
/// fonts across pages.
///
/// # Errors
/// As page compilation and [`TextPage::from_positioned`].
pub fn compile_page_with_text(
    document: &pagura_document::Document,
    page: &pagura_document::Page,
    provider: Option<&dyn pagura_text::Provider>,
) -> Result<(crate::DisplayList, TextPage)> {
    let fonts = std::sync::Mutex::new(cache::FontCache::new(document.limits()));
    compile_with_text(document, page, provider, &fonts)
}

/// Extracts a page's positioned text; painting commands are discarded and
/// rendering output is never modified. Conventions as
/// [`compile_page_with_text`].
///
/// # Errors
/// As [`compile_page_with_text`].
pub fn text_page(
    document: &pagura_document::Document,
    page: &pagura_document::Page,
    provider: Option<&dyn pagura_text::Provider>,
) -> Result<TextPage> {
    compile_page_with_text(document, page, provider).map(|(_, text)| text)
}

pub(crate) fn compile_with_text(
    document: &pagura_document::Document,
    page: &pagura_document::Page,
    provider: Option<&dyn pagura_text::Provider>,
    fonts: &std::sync::Mutex<cache::FontCache>,
) -> Result<(crate::DisplayList, TextPage)> {
    let output = Rc::new(RefCell::new(Output::default()));
    let mut compiler = Compiler::new(document, page)?;
    compiler.compilation = Rc::new(super::super::Compilation {
        colors: crate::icc::Cache::default(),
        provider,
        fonts: Some(fonts),
    });
    compiler.extraction = Some(Extraction {
        output: Rc::clone(&output),
        path: Arc::from([]),
        spans: Vec::new(),
    });
    compiler.page(page).map_err(|e| e.in_object(page.id()))?;
    let text = std::mem::take(&mut output.borrow_mut().text);
    let text = TextPage::from_positioned(text, LayoutOptions::default(), document.limits())?;
    Ok((compiler.list, text))
}

impl Compiler<'_> {
    pub(in crate::interpreter) fn inherit_extraction(&self, child: &mut Self, hidden: bool) {
        if let Some(extraction) = &self.extraction {
            child.extraction = Some(extraction.clone());
            child.visible &= !hidden;
        }
    }

    pub(super) fn extract_font(
        &mut self,
        font: &cache::CachedFont,
        object: Option<ObjectId>,
        dictionary: &Dictionary,
    ) -> Result<()> {
        let Some(extract) = &self.extraction else {
            return Ok(());
        };
        let mut output = extract.output.borrow_mut();
        let key = std::ptr::from_ref(font).addr();
        if output.fonts.contains_key(&key) {
            return Ok(());
        }
        let base_name = dictionary
            .get(b"BaseFont")
            .and_then(|v| self.document.resolve(v).ok())
            .and_then(Value::as_name)
            .cloned();
        output.charge(
            256 + base_name.as_ref().map_or(0, Name::len),
            self.document.limits(),
        )?;
        let bbox = font_box(self.document, dictionary, font.font.kind());
        let index = output.text.fonts.len();
        output.text.fonts.push(TextFont {
            object,
            kind: font.font.kind(),
            base_name,
        });
        output.fonts.insert(key, Font { index, bbox });
        Ok(())
    }
    pub(super) fn extract_show(&mut self) -> Result<()> {
        if let Some(extract) = &self.extraction {
            let mut output = extract.output.borrow_mut();
            output.show = output
                .show
                .checked_add(1)
                .ok_or(Error::limit("text string count exceeded"))?;
        }
        Ok(())
    }
    pub(super) fn extract_glyph(
        &mut self,
        font: &cache::CachedFont,
        glyph: &Glyph,
        matrix: Transform,
        transform: Transform,
    ) -> Result<()> {
        let Some(extract) = &self.extraction else {
            return Ok(());
        };
        let mut output = extract.output.borrow_mut();
        if output.text.glyphs.len() >= self.document.limits().value_nodes {
            return Err(Error::limit("positioned glyph count exceeded"));
        }
        output.charge(
            size_of::<PositionedGlyph>() * 2 + glyph.unicode.as_ref().map_or(0, String::len),
            self.document.limits(),
        )?;
        let info = output
            .fonts
            .get(&std::ptr::from_ref(font).addr())
            .ok_or(Error::syntax("missing extraction font"))?;
        let text = &self.state.text;
        let trm = Transform::new(
            text.size * text.horizontal,
            0.0,
            0.0,
            text.size,
            0.0,
            text.rise,
        )
        .then(matrix)
        .then(self.state.ctm);
        let origin = trm.apply(Point::default());
        let vertical = glyph.vertical;
        let unit = if vertical.is_some() {
            Point::new(0.0, -1.0)
        } else {
            Point::new(1.0, 0.0)
        };
        let tip = trm.apply(unit);
        let axis = Point::new(tip.x - origin.x, tip.y - origin.y);
        let perp = trm.apply(Point::new(-unit.y, unit.x));
        let em = (perp.x - origin.x).hypot(perp.y - origin.y);
        let advance = vertical.map_or(Point::new(glyph.advance, 0.0), |v| {
            Point::new(0.0, v.advance)
        });
        let advance_end = trm.apply(advance);
        let (bbox, cell_transform) = if let GlyphShape::Type3 { matrix, .. } = &glyph.shape {
            (
                info.bbox.unwrap_or(cell(0.0, 0.0, 1000.0, 1000.0)),
                matrix.then(transform),
            )
        } else {
            let (bottom, top) = info
                .bbox
                .map_or((0.0, 1.0), |b| (b.y0 / 1000.0, b.y1 / 1000.0));
            let bbox = vertical.map_or(cell(0.0, bottom, glyph.advance, top), |v| {
                cell(0.0, 0.0, glyph.advance, -v.advance)
            });
            (bbox, transform)
        };
        let quad = Quad(
            [
                Point::new(bbox.x0, bbox.y0),
                Point::new(bbox.x1, bbox.y0),
                Point::new(bbox.x1, bbox.y1),
                Point::new(bbox.x0, bbox.y1),
            ]
            .map(|p| cell_transform.apply(p)),
        );
        if !quad
            .0
            .into_iter()
            .chain([origin, axis, advance_end])
            .all(Point::is_finite)
            || !em.is_finite()
        {
            return Err(Error::syntax("text selection geometry overflow"));
        }
        let record = PositionedGlyph {
            unicode: glyph.unicode.clone(),
            font: info.index,
            size: text.size,
            writing_mode: font.font.writing_mode(),
            quad,
            origin,
            axis,
            em,
            advance_end,
            render_mode: text.mode,
            hidden: !self.visible,
            marked_content: Arc::clone(&extract.path),
            show: output.show,
        };
        output.text.glyphs.push(record);
        Ok(())
    }
    pub(in crate::interpreter) fn extract_begin(
        &mut self,
        word: &[u8],
        v: &[Value],
        resources: &Dictionary,
    ) -> Result<()> {
        let Some(extract) = &mut self.extraction else {
            return Ok(());
        };
        let tag = v
            .first()
            .and_then(Value::as_name)
            .cloned()
            .ok_or(Error::syntax("marked content tag expected"))?;
        let dictionary = if word == b"BDC" {
            let properties = v
                .get(1)
                .ok_or(Error::syntax("marked content properties expected"))?;
            let properties = if let Some(name) = properties.as_name() {
                resource(self.document, resources, b"Properties", name.as_bytes())?
            } else {
                properties
            };
            Some(self.document.resolve_dictionary(properties)?)
        } else {
            None
        };
        let actual_text = property(self.document, dictionary, b"ActualText")?;
        let alt = property(self.document, dictionary, b"Alt")?;
        let mut output = extract.output.borrow_mut();
        if output.text.marked.len() >= self.document.limits().value_nodes {
            return Err(Error::limit("marked content count exceeded"));
        }
        output.charge(
            256 + tag.len()
                + actual_text.as_ref().map_or(0, String::len)
                + alt.as_ref().map_or(0, String::len)
                + extract.path.len() * 16,
            self.document.limits(),
        )?;
        let id = output.text.marked.len();
        let content = Arc::new(MarkedContent {
            id,
            tag,
            mcid: dictionary
                .and_then(|d| d.get(b"MCID"))
                .and_then(|v| self.document.resolve(v).ok())
                .and_then(Value::as_integer),
            actual_text,
            alt,
        });
        let start = output.text.glyphs.len();
        output.text.marked.push(MarkedSpan {
            content: Arc::clone(&content),
            ancestors: Arc::clone(&extract.path),
            glyphs: start..start,
            hidden: !self.visible,
        });
        let mut path = extract.path.to_vec();
        path.push(content);
        extract.path = path.into();
        extract.spans.push(id);
        Ok(())
    }
    pub(in crate::interpreter) fn extract_end(&mut self) -> Result<()> {
        let Some(extract) = &mut self.extraction else {
            return Ok(());
        };
        let id = extract
            .spans
            .pop()
            .ok_or(Error::syntax("unbalanced extraction marked content"))?;
        let mut output = extract.output.borrow_mut();
        let end = output.text.glyphs.len();
        output.text.marked[id].glyphs.end = end;
        extract.path = Arc::clone(&output.text.marked[id].ancestors);
        Ok(())
    }
}
impl Output {
    fn charge(&mut self, bytes: usize, limits: pagura_core::Limits) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= limits.document_bytes)
            .ok_or(Error::limit("text capture byte budget exceeded"))?;
        Ok(())
    }
}
fn font_box(
    document: &pagura_document::Document,
    dictionary: &Dictionary,
    kind: pagura_text::FontKind,
) -> Option<Rect> {
    let dictionary = if kind == pagura_text::FontKind::Type0 {
        let first = document
            .resolve(dictionary.get(b"DescendantFonts")?)
            .ok()?
            .as_array()?
            .first()?;
        document.resolve_dictionary(first).ok()?
    } else {
        dictionary
    };
    let dictionary = if kind == pagura_text::FontKind::Type3 {
        dictionary
    } else {
        document
            .resolve_dictionary(dictionary.get(b"FontDescriptor")?)
            .ok()?
    };
    document
        .resolve(dictionary.get(b"FontBBox")?)
        .ok()?
        .as_array()
        .and_then(Rect::from_array)
}
fn property(
    document: &pagura_document::Document,
    dictionary: Option<&Dictionary>,
    name: &[u8],
) -> Result<Option<String>> {
    dictionary
        .and_then(|d| d.get(name))
        .map(|v| {
            let value = document.resolve(v)?;
            let bytes = value
                .as_string()
                .ok_or(Error::syntax("marked text property must be a text string"))?;
            pagura_text::decode_text_string(bytes, document.limits())
        })
        .transpose()
}

fn cell(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect { x0, y0, x1, y1 }
}
