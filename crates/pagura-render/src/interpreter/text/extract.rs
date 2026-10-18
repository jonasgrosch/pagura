//! Optional semantic output from the existing interpreter (9.4.4; 14.6).
use super::{
    Arc, Compiler, Dictionary, Error, Glyph, GlyphShape, Rc, RefCell, Result, Transform, Value,
    cache, resource,
};
use pagura_core::{Name, ObjectId, Rect};
use pagura_extract::{
    LayoutOptions, MarkedContent, MarkedSpan, PositionedGlyph, PositionedText, Quad, TextFont,
    TextPage,
};
use pagura_raster::Point;
use std::collections::BTreeMap;

mod unicode;

#[derive(Clone)]
pub(in crate::interpreter) struct Extraction {
    output: Rc<RefCell<Output>>,
    path: Arc<[Arc<MarkedContent>]>,
    spans: Vec<usize>,
    container: ObjectId,
}
#[derive(Default)]
struct Output {
    text: PositionedText,
    fonts: BTreeMap<usize, Font>,
    bytes: usize,
    show: usize,
    name_work: usize,
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

/// Extracts the actual glyph positions of one stored annotation appearance.
/// Coordinates include appearance fitting and remain in default page user space.
/// Page content and other annotations are not interpreted.
///
/// # Errors
/// As appearance rendering and bounded text extraction.
pub fn appearance_text(
    document: &pagura_document::Document,
    page: &pagura_document::Page,
    appearance: ObjectId,
    rect: Rect,
    provider: Option<&dyn pagura_text::Provider>,
) -> Result<PositionedText> {
    let output = Rc::new(RefCell::new(Output::default()));
    let mut compiler = Compiler::new(document, page)?;
    compiler.compilation = Rc::new(super::super::Compilation {
        colors: crate::icc::Cache::default(),
        provider,
        fonts: None,
        filler: None,
    });
    compiler.extraction = Some(Extraction {
        output: Rc::clone(&output),
        path: Arc::from([]),
        spans: Vec::new(),
        container: appearance,
    });
    compiler.paint_appearance(
        crate::interpreter::forms::FormBody::Stored(document.object(appearance)?),
        rect,
        page.resources(),
        true,
    )?;
    let text = std::mem::take(&mut output.borrow_mut().text);
    Ok(text)
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
        filler: None,
    });
    compiler.extraction = Some(Extraction {
        output: Rc::clone(&output),
        path: Arc::from([]),
        spans: Vec::new(),
        container: page.id(),
    });
    compiler.page(page).map_err(|e| e.in_object(page.id()))?;
    let text = std::mem::take(&mut output.borrow_mut().text);
    let text = TextPage::from_positioned(text, LayoutOptions::default(), document.limits())?
        .with_structure(document, page)?;
    Ok((compiler.list, text))
}

impl Compiler<'_> {
    pub(in crate::interpreter) fn inherit_extraction(&self, child: &mut Self, hidden: bool) {
        if let Some(extraction) = &self.extraction {
            let mut inherited = extraction.clone();
            inherited.container = child.forms.last().copied().unwrap_or(extraction.container);
            child.extraction = Some(inherited);
            child.visible &= !hidden;
        }
    }

    pub(in crate::interpreter) fn extract_form_sequence(
        &mut self,
        object: &pagura_document::IndirectObject,
        resources: &Dictionary,
    ) -> Result<()> {
        let start = self
            .extraction
            .as_ref()
            .map(|e| e.output.borrow().text.glyphs.len());
        self.sequence(&[object], resources)?;
        if let Some(extract) = &self.extraction
            && let Some(start) = start
        {
            let mut output = extract.output.borrow_mut();
            output.charge(
                size_of::<pagura_extract::TextObject>(),
                self.document.limits(),
            )?;
            let end = output.text.glyphs.len();
            output.text.objects.push(pagura_extract::TextObject {
                marked_content: Arc::clone(&extract.path),
                object: object.id(),
                glyphs: start..end,
                hidden: !self.visible,
            });
        }
        Ok(())
    }

    pub(in crate::interpreter) fn extract_graphic_object(
        &self,
        object: ObjectId,
        hidden: bool,
    ) -> Result<()> {
        if let Some(extract) = &self.extraction {
            let mut output = extract.output.borrow_mut();
            output.charge(
                size_of::<pagura_extract::TextObject>(),
                self.document.limits(),
            )?;
            let end = output.text.glyphs.len();
            output.text.objects.push(pagura_extract::TextObject {
                object,
                glyphs: end..end,
                hidden: hidden || !self.visible,
                marked_content: Arc::clone(&extract.path),
            });
        }
        Ok(())
    }

    // Called only by the renderer's annotation walk, after its flags and OC
    // decisions. Appearance operators remain excluded from page glyph records.
    pub(in crate::interpreter) fn extract_annotation(
        &self,
        value: &Value,
        hidden: bool,
    ) -> Result<()> {
        if let Some(object) = value.as_reference() {
            self.extract_graphic_object(object, hidden)?;
        }
        Ok(())
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
        let unicode = output.unicode(
            std::ptr::from_ref(font).addr(),
            glyph,
            self.document.limits(),
        )?;
        output.charge(
            size_of::<PositionedGlyph>() * 2 + unicode.as_deref().map_or(0, str::len),
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
                info.bbox.map_or(cell(0.0, 0.0, 1000.0, 1000.0), Cell::from),
                matrix.then(transform),
            )
        } else {
            let (bottom, top) = info
                .bbox
                .map_or((0.0, 1.0), |b| (b.y0() / 1000.0, b.y1() / 1000.0));
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
            unicode: unicode.map(std::borrow::Cow::into_owned),
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
            let result = (|| {
                let properties = if let Some(name) = properties.as_name() {
                    resource(self.document, resources, b"Properties", name.as_bytes())?
                } else {
                    properties
                };
                self.document.resolve_dictionary(properties)
            })();
            match result {
                Ok(dictionary) => Some(dictionary),
                Err(error) => {
                    ignore_metadata_fault(self.document, error, self.position)?;
                    None
                }
            }
        } else {
            None
        };
        let actual_text = property(self.document, dictionary, b"ActualText", self.position)?;
        let alt = property(self.document, dictionary, b"Alt", self.position)?;
        // Expansion text is metadata for speech, not a page replacement.
        let _ = property(self.document, dictionary, b"E", self.position)?;
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
            container: Some(extract.container),
            mcid: marked_id(self.document, dictionary)?,
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
    fn unicode<'a>(
        &mut self,
        font: usize,
        glyph: &'a Glyph,
        limits: pagura_core::Limits,
    ) -> Result<Option<std::borrow::Cow<'a, str>>> {
        let info = self
            .fonts
            .get(&font)
            .ok_or(Error::syntax("missing extraction font"))?;
        let base = self
            .text
            .fonts
            .get(info.index)
            .and_then(|font| font.base_name.as_ref())
            .map_or(b"".as_slice(), Name::as_bytes);
        unicode::recover(glyph, base, &mut self.name_work, limits)
    }

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
    position: pagura_core::Position,
) -> Result<Option<String>> {
    let Some(value) = dictionary.and_then(|d| d.get(name)) else {
        return Ok(None);
    };
    let result = (|| {
        let value = document.resolve(value)?;
        if value.is_null() {
            return Ok(None);
        }
        let message = match name {
            b"ActualText" => "non-string marked ActualText ignored (14.9.4)",
            b"Alt" => "non-string marked Alt ignored (14.9.3)",
            _ => "non-string marked E ignored (14.9.5)",
        };
        let bytes = value.as_string().ok_or(Error::syntax(message))?;
        // E is speech metadata, not replacement text in either exported view.
        // Validate its type without repeatedly decoding an unused resource.
        if name == b"E" {
            return Ok(None);
        }
        let mut strict = pagura_core::Diagnostics::new(pagura_core::Recovery::Strict);
        pagura_core::text_string::decode(bytes, &document.limits(), &mut strict).map(Some)
    })();
    match result {
        Err(error) => {
            ignore_metadata_fault(document, error, position)?;
            Ok(None)
        }
        result => result,
    }
}

fn ignore_metadata_fault(
    document: &pagura_document::Document,
    error: Error,
    position: pagura_core::Position,
) -> Result<()> {
    if !matches!(
        error.kind,
        pagura_core::ErrorKind::Syntax
            | pagura_core::ErrorKind::MissingObject
            | pagura_core::ErrorKind::Cycle
    ) {
        return Err(error);
    }
    let mut diagnostic =
        pagura_core::Diagnostic::new(pagura_extract::repairs::MARKED_PROPERTY, error.message)
            .at(position);
    if let pagura_core::Position::Stream { object, .. } = position {
        diagnostic = diagnostic.in_object(object);
    }
    crate::interpreter::repair(document, diagnostic)
}

fn marked_id(
    document: &pagura_document::Document,
    dictionary: Option<&Dictionary>,
) -> Result<Option<i64>> {
    let Some(value) = dictionary.and_then(|d| d.get(b"MCID")) else {
        return Ok(None);
    };
    match document.resolve(value) {
        Ok(value) => Ok(value.as_integer().filter(|n| *n >= 0)),
        Err(error)
            if matches!(
                error.kind,
                pagura_core::ErrorKind::Syntax
                    | pagura_core::ErrorKind::MissingObject
                    | pagura_core::ErrorKind::Cycle
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// A glyph cell in text space. Unlike a [`Rect`], its corners keep the
/// direction of the advance, so the selection quad follows the writing
/// direction (a negative advance gives a reversed cell).
#[derive(Clone, Copy)]
struct Cell {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

fn cell(x0: f64, y0: f64, x1: f64, y1: f64) -> Cell {
    Cell { x0, y0, x1, y1 }
}

impl From<Rect> for Cell {
    fn from(rect: Rect) -> Self {
        cell(rect.x0(), rect.y0(), rect.x1(), rect.y1())
    }
}

#[cfg(test)]
mod tests {
    use pagura_core::{
        Limits, Recovery, Value,
        syntax::{Origin, Parser},
    };
    use pagura_document::{Document, OpenOptions};
    use pagura_testkit::{fixture, fixture_path};

    fn expected(name: &str, extension: &str) -> Value {
        let bytes = fixture("text", &format!("{name}.{extension}"));
        Parser::new(&bytes, Origin::Buffer, Limits::default())
            .parse()
            .unwrap()
    }
    fn integer(value: &Value) -> usize {
        usize::try_from(value.as_integer().unwrap()).unwrap()
    }
    fn string(value: &Value) -> &str {
        std::str::from_utf8(value.as_string().unwrap()).unwrap()
    }
    fn names() -> Vec<&'static str> {
        include_str!("../../../../../corpus/manifests/text.json")
            .split("\"id\": \"text/")
            .skip(1)
            .filter_map(|s| s.split('"').next())
            .filter(|s| s.starts_with("tagged-"))
            .collect()
    }
    #[test]
    fn tagged_copy_ranges_quads_and_parent_ownership_match_independent_graphs() {
        let names = names();
        assert_eq!(names.len(), 58);
        for name in names {
            if fixture_path("text", &format!("{name}.error")).exists() {
                continue;
            }
            let doc = Document::open(
                fixture("text", &format!("{name}.pdf")),
                &OpenOptions::default(),
            )
            .unwrap();
            let page = doc.page(0).unwrap();
            let (list, text) = super::compile_page_with_text(&doc, page, None)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                text.text().as_bytes(),
                fixture("text", &format!("{name}.text")),
                "{name}"
            );
            assert_eq!(
                text.content_text().as_bytes(),
                fixture("text", &format!("{name}.content")),
                "{name}"
            );
            check_logical(name, &text);
            check_glyphs(name, &text);
            let relaid = text
                .with_layout(pagura_extract::LayoutOptions {
                    include_hidden: true,
                    space_gap: 1.0,
                    ..pagura_extract::LayoutOptions::default()
                })
                .unwrap();
            assert_eq!(
                text.logical().map(std::ptr::from_ref),
                relaid.logical().map(std::ptr::from_ref)
            );

            let normal = crate::compile_page(&doc, page).unwrap();
            let options = crate::RenderOptions::default();
            assert_eq!(
                list.render(&options).unwrap().data(),
                normal.render(&options).unwrap().data(),
                "{name}: paint changed"
            );
            assert!(
                text.retained_bytes()
                    >= size_of_val(text.glyphs()) + size_of_val(text.reading_characters())
            );
            if name == "tagged-nested-order" {
                let found = text
                    .find("First Second", pagura_extract::SearchOptions::default())
                    .unwrap();
                assert_eq!(found.len(), 1);
                assert_eq!(found[0].quads.first(), Some(&text.glyphs()[0].quad));
                assert_eq!(found[0].quads.len(), 11);
                assert_eq!(
                    text.find("Second First", pagura_extract::SearchOptions::default())
                        .unwrap(),
                    [] as [pagura_extract::TextMatch; 0]
                );
            }
            if name == "tagged-all-artifact" {
                assert_eq!(text.text(), "Footer");
                assert_eq!(text.logical().unwrap().text(), "");
                assert_eq!(
                    text.find("Footer", pagura_extract::SearchOptions::default())
                        .unwrap()[0]
                        .quads
                        .len(),
                    6
                );
            }
        }
    }
    fn check_logical(name: &str, text: &pagura_extract::TextPage) {
        if let Some(logical) = text.logical() {
            assert_eq!(
                logical.text().as_bytes(),
                fixture("text", &format!("{name}.logical")),
                "{name}"
            );
            let chars = expected(name, "logical-characters");
            let chars = chars.as_array().unwrap();
            assert_eq!(logical.characters().len(), chars.len(), "{name}");
            for (character, row) in logical.characters().iter().zip(chars) {
                let row = row.as_array().unwrap();
                assert_eq!(character.unicode, string(&row[0]).chars().next());
                assert_eq!(character.range, integer(&row[1])..integer(&row[2]));
                assert_eq!(
                    character.glyphs,
                    integer(&row[3])..integer(&row[4]),
                    "{name}"
                );
                assert_eq!(character.inserted, row[5].as_bool().unwrap());
            }
            let rows = expected(name, "structure");
            let rows = rows.as_array().unwrap();
            let elements = logical.structure();
            assert_eq!(elements.len(), rows.len(), "{name}");
            for (element, row) in elements.iter().zip(rows) {
                let row = row.as_array().unwrap();
                assert_eq!(
                    element.object.unwrap().number as usize,
                    integer(&row[0]),
                    "{name}"
                );
                assert_eq!(
                    element.parent,
                    usize::try_from(row[1].as_integer().unwrap()).ok()
                );
                assert_eq!(element.role.as_bytes(), string(&row[2]).as_bytes());
                assert_eq!(
                    element
                        .standard_type
                        .as_ref()
                        .map(pagura_core::Name::as_bytes),
                    row[3].as_string()
                );
                assert_eq!(
                    element.namespace == pagura_extract::StructureNamespace::Pdf20,
                    row[6].as_bool().unwrap()
                );
                assert_eq!(
                    element.range,
                    integer(&row[4])..integer(&row[5]),
                    "{name}: {:?}",
                    element.role
                );
            }
        } else {
            assert!(
                fixture_path("text", &format!("{name}.unstructured")).exists(),
                "{name}"
            );
        }
    }
    fn check_glyphs(name: &str, text: &pagura_extract::TextPage) {
        let rows = expected(name, "glyphs");
        let rows = rows.as_array().unwrap();
        assert_eq!(text.glyphs().len(), rows.len());
        for (glyph, row) in text.glyphs().iter().zip(rows) {
            let row = row.as_array().unwrap();
            assert_eq!(glyph.unicode.as_deref(), Some(string(&row[0])));
            for (point, xy) in glyph
                .quad
                .0
                .iter()
                .zip(row[1].as_array().unwrap().as_chunks::<2>().0)
            {
                assert!((point.x - xy[0].as_number().unwrap()).abs() < 1e-9);
                assert!((point.y - xy[1].as_number().unwrap()).abs() < 1e-9);
            }
            if let Some(mcid) = row[3].as_integer() {
                let mark = glyph
                    .marked_content
                    .iter()
                    .find(|m| m.mcid.is_some())
                    .unwrap();
                assert_eq!(mark.mcid, Some(mcid));
                assert_eq!(mark.container.unwrap().number as usize, integer(&row[2]));
            } else {
                assert!(glyph.marked_content.iter().all(|m| m.mcid.is_none()));
            }
        }
    }
    #[test]
    fn null_structure_entries_are_absent_without_repairs_in_strict_mode() {
        for name in [
            "tagged-null-optionals",
            "tagged-indirect-null-optionals",
            "tagged-null-kids",
        ] {
            let doc = Document::open(
                fixture("text", &format!("{name}.pdf")),
                &OpenOptions {
                    recovery: Recovery::Strict,
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let text = super::text_page(&doc, doc.page(0).unwrap(), None).unwrap();
            assert_eq!(
                text.text().as_bytes(),
                fixture("text", &format!("{name}.text"))
            );
            assert_eq!(doc.diagnostics(), []);
        }
    }
    #[test]
    fn malformed_marked_properties_are_reported_without_changing_page_glyphs() {
        for name in names()
            .into_iter()
            .filter(|n| fixture_path("text", &format!("{n}.property-repair")).exists())
        {
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let document = Document::open(
                    fixture("text", &format!("{name}.pdf")),
                    &OpenOptions {
                        recovery,
                        ..OpenOptions::default()
                    },
                )
                .unwrap();
                let page = document.page(0).unwrap();
                assert!(crate::compile_page(&document, page).is_ok());
                let result = super::text_page(&document, page, None);
                let tag = fixture("text", &format!("{name}.property-repair"));
                if recovery == Recovery::Strict {
                    assert_eq!(result.unwrap_err().repair.unwrap().as_str().as_bytes(), tag);
                } else {
                    let text = result.unwrap();
                    assert_eq!(
                        text.text().as_bytes(),
                        fixture("text", &format!("{name}.text"))
                    );
                    assert!(
                        document
                            .diagnostics()
                            .iter()
                            .any(|d| d.tag().as_bytes() == tag)
                    );
                    assert!(text.structure_fallback().is_none());
                    check_glyphs(name, &text);
                }
            }
        }
    }

    #[test]
    fn compilation_budget_errors_are_not_structure_repairs() {
        for recovery in [Recovery::Strict, Recovery::Repair] {
            let document = Document::open(
                fixture("text", "tagged-nested-order.pdf"),
                &OpenOptions {
                    recovery,
                    limits: Limits {
                        operations: 1,
                        ..Limits::default()
                    },
                    ..OpenOptions::default()
                },
            )
            .unwrap();
            let page = document.page(0).unwrap();
            let ordinary = crate::compile_page(&document, page).unwrap_err();
            let extraction = super::text_page(&document, page, None).unwrap_err();
            assert_eq!(ordinary.kind, pagura_core::ErrorKind::Limit);
            assert_eq!(extraction, ordinary);
            assert_eq!(extraction.repair, None);
            assert!(
                document
                    .diagnostics()
                    .iter()
                    .all(|d| d.repair() != Some(pagura_extract::repairs::STRUCTURE_FALLBACK))
            );
        }
    }

    #[test]
    fn structure_failures_report_geometric_fallback_and_strict_rejection() {
        for name in names()
            .into_iter()
            .filter(|n| fixture_path("text", &format!("{n}.error")).exists())
        {
            let expected = fixture("text", &format!("{name}.error"));
            for recovery in [Recovery::Strict, Recovery::Repair] {
                let doc = Document::open(
                    fixture("text", &format!("{name}.pdf")),
                    &OpenOptions {
                        recovery,
                        ..Default::default()
                    },
                )
                .unwrap();
                let result = super::text_page(&doc, doc.page(0).unwrap(), None);
                if recovery == Recovery::Strict {
                    let error = result.unwrap_err();
                    assert_eq!(
                        error.repair.unwrap().as_str().as_bytes(),
                        fixture("text", &format!("{name}.repair"))
                    );
                    assert!(error.object.is_some());
                    assert_ne!(error.position, pagura_core::Position::Unknown);
                } else {
                    let text = result.unwrap();
                    assert!(text.structure().is_none());
                    let error = text.structure_fallback().unwrap();
                    let kind = error
                        .feature()
                        .map_or(error.kind.name(), pagura_core::Feature::as_str);
                    assert_eq!(kind.as_bytes(), expected, "{name}: {error}");
                    assert_eq!(
                        text.text().as_bytes(),
                        fixture("text", &format!("{name}.fallback")),
                        "{name}"
                    );
                    let geometric = text
                        .with_layout(pagura_extract::LayoutOptions::default())
                        .unwrap();
                    assert_eq!(text.reading_characters(), geometric.reading_characters());
                    assert_eq!(text.lines(), geometric.lines());
                    assert_eq!(text.words(), geometric.words());
                    assert_eq!(text.retained_bytes(), geometric.retained_bytes());
                    let reports = doc.diagnostics();
                    let report = reports
                        .iter()
                        .find(|d| d.repair() == Some(pagura_extract::repairs::STRUCTURE_FALLBACK))
                        .unwrap();
                    assert_eq!(report.message, error.message);
                    assert_eq!(report.position, error.position);
                    assert_eq!(report.object, error.object);
                    if let Some(feature) = error.feature() {
                        assert!(report.message.contains(feature.as_str()));
                    }
                    assert_eq!(text.clone().structure_fallback(), Some(error));
                    assert_eq!(geometric.structure_fallback(), Some(error));
                    assert!(text.logical().is_none());
                }
                assert!(crate::compile_page(&doc, doc.page(0).unwrap()).is_ok());
            }
        }
    }
}
