//! Appearance data for in-memory form values (ISO 32000-2, 12.7.4.3–12.7.5).
//!
//! This API neither mutates nor serializes the source document. Resource
//! references remain relative to `FormFiller::document`. Generated programs
//! are ordinary Form XObjects, suitable for the annotation appearance path.
//! Layout is bounded reader policy: greedy whitespace wrapping, 1.2 em default
//! leading, a 12 pt auto-size ceiling, 1/64 pt size steps when shrinking,
//! descriptor ascent/descent (0.8/-0.2 em when absent) and two units of padding.
//! Centered comb text stays on the cell grid; an odd spare cell goes at the right.
//! Generated text starts from default text state before applying DA.
//! Passwords use asterisks; no actions, rich text or XFA are evaluated.
mod buttons;
mod choice;
mod compatibility;
mod da;
pub mod features;
mod font;
mod layout;
mod style;
mod update;
mod variable;

use da::DefaultAppearance;
use font::Font;
use pagura_core::{Dictionary, Error, Limits, Name, ObjectId, Rect, Result, Value};
use pagura_document::{
    Document,
    forms::{Field, FieldKind, FormFiller, Widget, WidgetState},
};
use pagura_text::Provider;
use style::Style;

type FontCache<'a> = Option<&'a std::sync::Mutex<crate::interpreter::text::cache::FontCache>>;

/// Policy for updating existing form appearances.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UpdatePolicy {
    /// Follow NeedAppearances and the specified Tx replacement/append rules.
    #[default]
    Specification,
    /// Keep unchanged cached appearances. For edited fields, allow named
    /// standard-font resources to be supplied, remove unmarked old text, and
    /// rebuild zero-area appearances using the widget bounds.
    PreserveExisting,
}

/// Normal appearance selected for one widget (12.5.5).
#[derive(Debug, Clone, PartialEq)]
pub enum Appearance {
    /// An existing indirect stream, selected using the replacement AS state.
    Existing(ObjectId),
    /// An undefined appearance state paints nothing (12.5.5).
    Hidden,
    /// An unencoded appearance program and its complete form dictionary.
    Generated(AppearanceStream),
}
/// An appearance stream kept as data, without allocating indirect object IDs.
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceStream {
    dictionary: Dictionary,
    content: Vec<u8>,
}
impl AppearanceStream {
    /// Complete Form XObject dictionary, including BBox and Resources.
    /// Filter, DecodeParms and Length are absent: content is decoded data.
    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }
    /// Content program, before any stream encoding.
    pub fn content(&self) -> &[u8] {
        &self.content
    }
    /// Estimated caller-owned storage; source references count only their IDs.
    pub fn retained_bytes(&self) -> usize {
        self.dictionary
            .retained_bytes()
            .saturating_add(self.content.capacity())
            .saturating_add(size_of::<Self>())
    }
}
/// One widget's prepared appearance. Widget indexes refer to the source form.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetAppearance {
    widget: usize,
    appearance: Appearance,
}
impl WidgetAppearance {
    /// Source widget index, preserving object identity separately from names.
    pub fn widget(&self) -> usize {
        self.widget
    }
    /// Selected original stream or generated data.
    pub fn appearance(&self) -> &Appearance {
        &self.appearance
    }
}
/// Prepares all normal widget appearances for a form session.
///
/// Unchanged fields retain AP unless NeedAppearances requests regeneration.
/// Changed text/choice fields replace only the Tx section of an existing AP;
/// existing resources win collisions with DR. Buttons always select an existing
/// state when AP exists; only an absent AP permits a generated ZaDb check mark.
/// Other field types, such as signatures, keep AP. Without AP, they and any
/// widget whose Rect has no area are hidden. No partial result is returned.
/// Storage is separately capped by document_bytes.
///
/// # Errors
/// Returns positioned syntax, font, limit and unsupported-capability errors.
/// Horizontal simple fonts with reversible single-character Unicode mappings
/// are supported. Oblique text matrices, rotated widgets, rich text, XFA,
/// beveled/inset borders, negative font sizes and appearance updates crossing
/// text/graphics scopes or containing inline images remain explicit
/// unsupported capabilities. DoNotScroll rejects text that will not visibly fit.
pub fn prepare(
    filler: &FormFiller<'_>,
    provider: Option<&dyn Provider>,
) -> Result<Vec<WidgetAppearance>> {
    let fonts = std::sync::Mutex::new(crate::interpreter::text::cache::FontCache::new(
        filler.document().limits(),
    ));
    let count = filler.form().widgets.len();
    let mut bytes = count
        .checked_mul(size_of::<WidgetAppearance>())
        .filter(|n| *n <= filler.document().limits().document_bytes)
        .ok_or(Error::limit(
            "prepared form appearance index budget exceeded",
        ))?;
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let appearance = prepare_widget_cached(filler, index, provider, Some(&fonts))?;
        bytes = bytes.saturating_add(match &appearance {
            Appearance::Generated(s) => s.retained_bytes(),
            _ => 0,
        });
        if bytes > filler.document().limits().document_bytes {
            return Err(Error::limit(
                "prepared form appearance storage budget exceeded",
            ));
        }
        result.push(WidgetAppearance {
            widget: index,
            appearance,
        });
    }
    Ok(result)
}
/// Prepares one widget, with the same contract as [`prepare`].
///
/// # Errors
/// As [`prepare`]; an invalid widget index returns InvalidArgument.
pub fn prepare_widget(
    filler: &FormFiller<'_>,
    index: usize,
    provider: Option<&dyn Provider>,
) -> Result<Appearance> {
    prepare_widget_cached(filler, index, provider, None)
}
pub(crate) fn prepare_widget_cached(
    filler: &FormFiller<'_>,
    index: usize,
    provider: Option<&dyn Provider>,
    fonts: FontCache<'_>,
) -> Result<Appearance> {
    prepare_widget_policy(
        filler,
        index,
        provider,
        (UpdatePolicy::Specification, fonts),
    )
}
/// Prepares one widget with an explicit appearance compatibility policy.
///
/// # Errors
/// As [`prepare_widget`]; unknown missing fonts and unsafe text clipping fail.
pub fn prepare_widget_with_policy(
    filler: &FormFiller<'_>,
    index: usize,
    provider: Option<&dyn Provider>,
    policy: UpdatePolicy,
) -> Result<Appearance> {
    prepare_widget_policy(filler, index, provider, (policy, None))
}
fn prepare_widget_policy(
    filler: &FormFiller<'_>,
    index: usize,
    provider: Option<&dyn Provider>,
    (policy, fonts): (UpdatePolicy, FontCache<'_>),
) -> Result<Appearance> {
    let widget = filler
        .form()
        .widgets
        .get(index)
        .ok_or(Error::invalid_argument("widget index is outside the form"))?;
    let field = &filler.form().fields[widget.field];
    prepare_inner(filler, index, widget, field, provider, (policy, fonts)).map_err(|mut e| {
        e = e.at(widget.position);
        if let Some(id) = widget.object {
            e = e.in_object(id);
        }
        e
    })
}
fn prepare_inner(
    filler: &FormFiller<'_>,
    index: usize,
    widget: &Widget<'_>,
    field: &Field<'_>,
    provider: Option<&dyn Provider>,
    (policy, fonts): (UpdatePolicy, FontCache<'_>),
) -> Result<Appearance> {
    let document = filler.document();
    let state = selected_state(filler, index, widget);
    let original = normal(document, widget, state)?;
    let button = matches!(field.kind, FieldKind::Button { .. });
    let variable = matches!(
        field.kind,
        FieldKind::Text { .. } | FieldKind::Choice { .. }
    );
    if let Some(original) = &original
        && (!variable
            || filler.get(widget.field).is_none()
                && (!filler.form().need_appearances || policy == UpdatePolicy::PreserveExisting))
    {
        return Ok(original.clone());
    }
    if original.is_none() && (!button && !variable || bounds(widget).is_err()) {
        return Ok(Appearance::Hidden);
    }
    if filler.form().has_xfa {
        return Err(Error::unsupported(
            pagura_document::features::XFA,
            "XFA appearance generation is unavailable",
        ));
    }
    if button {
        return buttons::check_mark(document, widget, state, field);
    }
    variable_appearance(
        filler,
        widget,
        field,
        original.as_ref(),
        provider,
        (policy, fonts),
    )
}
fn variable_appearance(
    filler: &FormFiller<'_>,
    widget: &Widget<'_>,
    field: &Field<'_>,
    original: Option<&Appearance>,
    provider: Option<&dyn Provider>,
    (policy, fonts): (UpdatePolicy, FontCache<'_>),
) -> Result<Appearance> {
    let document = filler.document();
    let original_id = match original {
        Some(Appearance::Existing(id)) => Some(*id),
        _ => None,
    };
    let mut source = original_id.map(|id| document.object(id)).transpose()?;
    let mut old = source.and_then(|o| o.dictionary());
    let mut bbox = match old {
        Some(old) => old
            .get(b"BBox")
            .map(|v| document.resolve(v))
            .transpose()?
            .and_then(Value::as_array)
            .and_then(Rect::from_array)
            .ok_or(Error::syntax("existing appearance has invalid BBox"))?,
        None => bounds(widget)?,
    };
    if policy == UpdatePolicy::PreserveExisting && (bbox.width() == 0.0 || bbox.height() == 0.0) {
        source = None;
        old = None;
        bbox = bounds(widget)?;
    }
    let existing = old
        .map(|d| optional_dictionary(document, d, b"Resources"))
        .transpose()?
        .flatten();
    let mut resources = update::resources(document, existing, filler.form().default_resources)?;
    let da = DefaultAppearance::parse(
        field
            .default_appearance
            .ok_or(Error::syntax("variable text field has no DA"))?,
        document.limits(),
    )?;
    if policy == UpdatePolicy::PreserveExisting {
        compatibility::supply_font(document, &mut resources, da.font.as_bytes())?;
    }
    let font = Font::load(document, &resources, da.font.as_bytes(), provider, fonts)?;
    let style = Style::read(document, widget.dictionary)?;
    let mut text = Program::new(document.limits());
    let area = inset(bbox, style.inset)?;
    text.text("/Tx BMC\nq\n")?;
    text.text(&format!(
        "{} {} {} {} re W n\n",
        area.x0(),
        area.y0(),
        area.width(),
        area.height()
    ))?;
    variable::write(&mut text, filler, widget.field, area, &font, &da)?;
    text.text("Q\nEMC\n")?;
    let text = text.finish();
    let content = if let Some(source) = source {
        update::replace_with_policy(
            &document.stream_data(source)?,
            &text,
            document.limits(),
            policy,
        )?
    } else {
        let mut out = Program::new(document.limits());
        style.paint(&mut out, bbox)?;
        out.bytes(&text)?;
        out.finish()
    };
    let mut dictionary = match old {
        Some(old) if old.retained_bytes() <= document.limits().document_bytes => old.clone(),
        Some(_) => {
            return Err(Error::limit(
                "appearance dictionary storage budget exceeded",
            ));
        }
        None => form_dictionary(bbox),
    };
    for key in [b"Length".as_slice(), b"Filter", b"DecodeParms"] {
        dictionary.remove(key);
    }
    dictionary.insert(Name::new(b"Resources"), Value::Dictionary(resources));
    finish(document, dictionary, content)
}
// Shared selection lets rendering cull an invisible or empty source AP before
// variable-text layout, just as ordinary annotation rendering does (12.5.5).
pub(crate) fn original(filler: &FormFiller<'_>, index: usize) -> Result<Option<Appearance>> {
    let widget = filler
        .form()
        .widgets
        .get(index)
        .ok_or(Error::invalid_argument("widget index is outside the form"))?;
    normal(
        filler.document(),
        widget,
        selected_state(filler, index, widget),
    )
}
fn selected_state<'a>(
    filler: &'a FormFiller<'_>,
    index: usize,
    widget: &'a Widget<'_>,
) -> Option<&'a Name> {
    filler
        .get(widget.field)
        .and_then(|c| c.widget_states().iter().find(|s| s.widget() == index))
        .map(WidgetState::state)
        .or(widget.appearance_state)
}
fn normal(
    document: &Document,
    widget: &Widget<'_>,
    state: Option<&Name>,
) -> Result<Option<Appearance>> {
    let Some(ap) = widget.dictionary.get(b"AP") else {
        return Ok(None);
    };
    let ap = document.resolve(ap)?;
    if ap.is_null() {
        return Ok(None);
    }
    let ap = ap
        .as_dictionary()
        .ok_or(Error::syntax("widget AP must be a dictionary"))?;
    let normal = ap.get(b"N").ok_or(Error::syntax("widget AP requires N"))?;
    if let Some(id) = normal.as_reference()
        && document.object(id)?.is_stream()
    {
        return Ok(Some(Appearance::Existing(id)));
    }
    let states = document.resolve_dictionary(normal)?;
    let state = state.ok_or(Error::syntax("widget appearance state is absent"))?;
    let Some(value) = states.get(state.as_bytes()) else {
        return Ok(Some(Appearance::Hidden));
    };
    let id = value
        .as_reference()
        .ok_or(Error::syntax("widget state appearance must be indirect"))?;
    if !document.object(id)?.is_stream() {
        return Err(Error::syntax("widget state appearance must be a stream"));
    }
    Ok(Some(Appearance::Existing(id)))
}
fn bounds(widget: &Widget<'_>) -> Result<Rect> {
    let (width, height) = (widget.rectangle.width(), widget.rectangle.height());
    Rect::from_corners(0.0, 0.0, width, height)
        .filter(|_| width > 0.0 && height > 0.0)
        .ok_or(Error::invalid_argument(
            "appearance widget rectangle must have finite positive area",
        ))
}
fn inset(bbox: Rect, padding: f64) -> Result<Rect> {
    let (x0, y0) = (bbox.x0() + padding, bbox.y0() + padding);
    let (x1, y1) = (bbox.x1() - padding, bbox.y1() - padding);
    // Padding wider than the box inverts it; that is no area, not a
    // rectangle to normalize.
    Rect::from_corners(x0, y0, x1, y1)
        .filter(|rect| x0 < x1 && y0 < y1 && rect.width().is_finite() && rect.height().is_finite())
        .ok_or(Error::invalid_argument(
            "widget has no area inside its text padding",
        ))
}
fn form_dictionary(bbox: Rect) -> Dictionary {
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"Type"), Value::name(b"XObject"));
    dictionary.insert(Name::new(b"Subtype"), Value::name(b"Form"));
    dictionary.insert(Name::new(b"FormType"), Value::Integer(1));
    dictionary.insert(
        Name::new(b"BBox"),
        Value::Array(
            [bbox.x0(), bbox.y0(), bbox.x1(), bbox.y1()]
                .map(Value::Real)
                .to_vec(),
        ),
    );
    dictionary
}
fn finish(document: &Document, dictionary: Dictionary, content: Vec<u8>) -> Result<Appearance> {
    let stream = AppearanceStream {
        dictionary,
        content,
    };
    if stream.retained_bytes() > document.limits().document_bytes {
        return Err(Error::limit("generated appearance storage budget exceeded"));
    }
    Ok(Appearance::Generated(stream))
}
struct Program {
    bytes: Vec<u8>,
    limit: usize,
}
impl Program {
    fn new(limits: Limits) -> Self {
        Self {
            bytes: Vec::new(),
            limit: limits
                .stream_bytes
                .min(limits.decoded_bytes)
                .min(limits.document_bytes),
        }
    }
    fn text(&mut self, text: &str) -> Result<()> {
        self.bytes(text.as_bytes())
    }
    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|n| n > self.limit)
        {
            return Err(Error::limit("appearance content budget exceeded"));
        }
        let total = self.bytes.len() + bytes.len();
        if total > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .max(256)
                .saturating_mul(2)
                .max(total)
                .min(self.limit);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|_| Error::limit("appearance content allocation failed"))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests;

// Null dictionary values are absent, including indirect null (7.3.9).
fn optional<'a>(
    document: &'a Document,
    dictionary: &'a Dictionary,
    key: &[u8],
) -> Result<Option<&'a Value>> {
    dictionary
        .get(key)
        .map(|v| document.resolve(v))
        .transpose()
        .map(|v| v.filter(|v| !v.is_null()))
}
fn optional_dictionary<'a>(
    document: &'a Document,
    dictionary: &'a Dictionary,
    key: &[u8],
) -> Result<Option<&'a Dictionary>> {
    optional(document, dictionary, key)?
        .map(|v| {
            v.as_dictionary()
                .ok_or(Error::syntax("form appearance entry must be a dictionary"))
        })
        .transpose()
}
