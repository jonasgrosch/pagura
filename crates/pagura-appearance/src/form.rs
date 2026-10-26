//! Shared appearances for source and in-memory form values (12.7.4.3–12.7.5).
//!
//! Nothing here mutates or serializes the source document. Resource
//! references remain relative to the source document. Generated programs
//! are ordinary Form XObjects, drawn like any annotation appearance.
//! Layout is bounded reader policy: greedy whitespace wrapping, 1.2 em default
//! leading, a 12 pt auto-size ceiling, 1/64 pt size steps when shrinking,
//! descriptor ascent/descent (0.8/-0.2 em when absent) and two units of padding.
//! Centered comb text stays on the cell grid; an odd spare cell goes at the right.
//! Generated text starts from default text state before applying DA.
//! Passwords use asterisks; no actions, rich text or XFA are evaluated.

mod buttons;
mod choice;
mod compatibility;
pub(crate) mod da;
mod filled;
pub(crate) mod layout;
mod style;
mod update;
mod variable;

pub use da::{TextStyle, default_text_style};
pub use filled::FilledWidgets;

pub(crate) use compatibility::supply_font;

use crate::fonts::Encoding as Font;
use crate::fonts::{FontSource, ProviderFonts};
use crate::program::Program;
use crate::stream::{Appearance, AppearanceStream, form_dictionary};
use crate::values::optional_dictionary;
use da::DefaultAppearance;
use pagura_core::{Dictionary, Error, Name, Rect, Result, Value};
use pagura_document::{
    Document,
    forms::{Field, FieldKind, FilledField, FormFiller, Widget, WidgetField, WidgetState},
};
use pagura_text::Provider;
use style::Style;

/// Policy for updating existing form appearances.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdatePolicy {
    /// Follow NeedAppearances and the specified Tx replacement/append rules.
    #[default]
    Specification,
    /// Keep unchanged cached appearances. For edited fields, allow named
    /// standard-font resources to be supplied, remove unmarked old text, and
    /// rebuild zero-area appearances using the widget bounds.
    PreserveExisting,
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

/// Prepares all normal widget appearances for a form session, loading fonts
/// with `provider` (ADR 0009) or only from the document for `None`.
///
/// Unchanged fields retain AP unless NeedAppearances requests regeneration.
/// Changed text/choice fields replace only the Tx section of an existing AP;
/// existing resources win collisions with DR. Buttons always select an existing
/// state when AP exists; only an absent AP permits a generated normal caption,
/// mark or icon.
/// Other field types, such as signatures, keep AP. Without AP, they and any
/// widget whose Rect has no area are hidden. No partial result is returned.
/// Storage is separately capped by document_bytes.
///
/// # Errors
/// Returns positioned syntax, font, limit and unsupported-capability errors.
/// Horizontal simple fonts with reversible single-character Unicode mappings
/// are supported. Oblique text matrices, rich text, XFA,
/// negative font sizes and appearance updates crossing
/// text/graphics scopes or containing inline images remain explicit
/// unsupported capabilities. DoNotScroll rejects text that will not visibly fit.
pub fn prepare(
    filler: &FormFiller<'_>,
    provider: Option<&dyn Provider>,
) -> Result<Vec<WidgetAppearance>> {
    let limits = filler.document().limits();
    let fonts = ProviderFonts::new(provider, &limits);
    let count = filler.form().widgets.len();
    let mut bytes = count
        .checked_mul(size_of::<WidgetAppearance>())
        .filter(|n| *n <= limits.document_bytes)
        .ok_or(Error::limit(
            "prepared form appearance index budget exceeded",
        ))?;
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let appearance = widget_appearance(filler, index, &fonts, UpdatePolicy::Specification)?;
        if let Appearance::Generated(stream) = &appearance {
            bytes = bytes.saturating_add(stream.retained_bytes());
        }
        if bytes > limits.document_bytes {
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
    prepare_widget_with_policy(filler, index, provider, UpdatePolicy::Specification)
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
    let fonts = ProviderFonts::new(provider, &filler.document().limits());
    widget_appearance(filler, index, &fonts, policy)
}

/// Generates both normal states for a checkbox/radio widget without an AP.
/// Caption fonts use `provider`, or only document resources for `None`.
///
/// The on-state name follows the filler's selection rules. Both programs use the
/// same generator as live preview, so saving Off does not remove the ability to
/// toggle the reopened field. The pair has one aggregate storage budget.
///
/// # Errors
/// Returns InvalidArgument for an existing AP or a non-toggle button. Other
/// errors are the source-form, appearance and resource-limit errors of [`prepare`].
pub fn prepare_button_states(
    filler: &FormFiller<'_>,
    index: usize,
    provider: Option<&dyn Provider>,
) -> Result<[(Name, AppearanceStream); 2]> {
    let widget = widget(filler, index)?;
    let field = field(filler, widget)?;
    let document = filler.document();
    if document.optional(widget.dictionary, b"AP")?.is_some() {
        return Err(Error::invalid_argument(
            "button already has appearance states",
        ));
    }
    if filler.form().has_xfa {
        return Err(Error::unsupported(
            pagura_document::features::XFA,
            "XFA appearance generation is unavailable",
        ));
    }
    let on = filler.button_on_state(index)?;
    let off = Name::new(b"Off");
    let fonts = ProviderFonts::new(provider, &document.limits());
    let context = Context {
        filler,
        index,
        widget,
        field,
        fonts: &fonts,
        policy: UpdatePolicy::Specification,
    };
    let off_stream = context.generate(None, Some(&off))?;
    let on_stream = context.generate(None, Some(&on))?;
    let bytes = off_stream
        .retained_bytes()
        .saturating_add(on_stream.retained_bytes())
        .saturating_add(off.len())
        .saturating_add(on.len())
        .saturating_add(size_of::<[(Name, AppearanceStream); 2]>());
    if bytes > document.limits().document_bytes {
        return Err(Error::limit(
            "button appearance states exceed storage budget",
        ));
    }
    Ok([(off, off_stream), (on, on_stream)])
}

/// One widget's appearance, with errors placed at the widget.
fn widget_appearance(
    filler: &FormFiller<'_>,
    index: usize,
    fonts: &dyn FontSource,
    policy: UpdatePolicy,
) -> Result<Appearance> {
    let widget = widget(filler, index)?;
    let field = field(filler, widget)?;
    let context = Context {
        filler,
        index,
        widget,
        field,
        fonts,
        policy,
    };
    context
        .appearance()
        .map_err(|error| error.at(widget.position).in_object_opt(widget.object))
}

fn widget<'f, 'd>(filler: &'f FormFiller<'d>, index: usize) -> Result<&'f Widget<'d>> {
    filler
        .form()
        .widgets
        .get(index)
        .ok_or(Error::invalid_argument("widget index is outside the form"))
}

fn field<'f, 'd>(filler: &'f FormFiller<'d>, widget: &Widget<'_>) -> Result<&'f Field<'d>> {
    filler
        .form()
        .fields
        .get(widget.field)
        .ok_or(Error::invalid_argument("widget field index is absent"))
}

/// The inputs of one widget's appearance.
struct Context<'a, 'd> {
    filler: &'a FormFiller<'d>,
    index: usize,
    widget: &'a Widget<'d>,
    field: &'a Field<'d>,
    fonts: &'a dyn FontSource,
    policy: UpdatePolicy,
}

impl Context<'_, '_> {
    fn document(&self) -> &Document {
        self.filler.document()
    }

    fn appearance(&self) -> Result<Appearance> {
        let state = selected_state(self.filler, self.index, self.widget);
        let original = normal(self.document(), self.widget, state)?;
        let button = matches!(self.field.kind, FieldKind::Button { .. });
        let variable = matches!(
            self.field.kind,
            FieldKind::Text { .. } | FieldKind::Choice { .. }
        );
        let keep = !variable
            || self.filler.get(self.widget.field).is_none()
                && (!self.filler.form().need_appearances
                    || self.policy == UpdatePolicy::PreserveExisting);
        if let Some(original) = &original
            && keep
        {
            return Ok(original.clone());
        }
        if original.is_none() && (!button && !variable || bounds(self.widget.rectangle).is_err()) {
            return Ok(Appearance::Hidden);
        }
        if self.filler.form().has_xfa {
            return Err(Error::unsupported(
                pagura_document::features::XFA,
                "XFA appearance generation is unavailable",
            ));
        }
        self.generate(original.as_ref(), state)
            .map(Appearance::Generated)
    }

    fn generate(
        &self,
        original: Option<&Appearance>,
        state: Option<&Name>,
    ) -> Result<AppearanceStream> {
        let state = if state.is_none()
            && matches!(
                self.field.kind,
                FieldKind::Button {
                    kind: pagura_document::forms::ButtonKind::CheckBox
                        | pagura_document::forms::ButtonKind::Radio,
                    ..
                }
            ) {
            self.filler.button_appearance_state(self.index)?
        } else {
            state
        };
        Generation {
            field: variable::Inputs {
                document: self.document(),
                kind: &self.field.kind,
                flags: self.field.flags,
                value: self.field.value.as_ref(),
                change: self.filler.get(self.widget.field).map(FilledField::value),
                dictionary: self.field.dictionary,
                default_appearance: self.field.default_appearance,
                justification: self.field.justification,
            },
            widget: self.widget.dictionary,
            bounds: self.widget.rectangle,
            default_resources: self.filler.form().default_resources,
            fonts: self.fonts,
            policy: self.policy,
        }
        .appearance(original, state)
    }
}

/// The same normal generator serves source display, explicit edits and persistence.
struct Generation<'a, 'd> {
    field: variable::Inputs<'a, 'd>,
    widget: &'d Dictionary,
    bounds: Rect,
    default_resources: Option<&'d Dictionary>,
    fonts: &'a dyn FontSource,
    policy: UpdatePolicy,
}

impl Generation<'_, '_> {
    /// A normal button stream or a new Tx section in the existing/new stream.
    fn appearance(
        &self,
        original: Option<&Appearance>,
        state: Option<&Name>,
    ) -> Result<AppearanceStream> {
        if let FieldKind::Button { kind, .. } = self.field.kind {
            return buttons::generate(self, *kind, state);
        }
        let document = self.field.document;
        let limits = document.limits();
        let original_id = match original {
            Some(Appearance::Existing(id)) => Some(*id),
            _ => None,
        };
        let mut source = original_id.map(|id| document.object(id)).transpose()?;
        let mut old = source.and_then(|object| object.dictionary());
        let mut bbox = match old {
            Some(old) => old
                .get(b"BBox")
                .map(|value| document.resolve(value))
                .transpose()?
                .and_then(Value::as_array)
                .and_then(Rect::from_array)
                .ok_or(Error::syntax("existing appearance has invalid BBox"))?,
            None => bounds(self.bounds)?,
        };
        if self.policy == UpdatePolicy::PreserveExisting
            && (bbox.width() == 0.0 || bbox.height() == 0.0)
        {
            source = None;
            old = None;
            bbox = bounds(self.bounds)?;
        }
        let existing = old
            .map(|dictionary| optional_dictionary(document, dictionary, b"Resources"))
            .transpose()?
            .flatten();
        let mut resources = update::resources(document, existing, self.default_resources)?;
        let da = DefaultAppearance::parse(
            self.field
                .default_appearance
                .ok_or(Error::syntax("variable text field has no DA"))?,
            &limits,
        )?;
        if self.policy == UpdatePolicy::PreserveExisting {
            supply_font(document, &mut resources, da.font.as_bytes())?;
        }
        let font = Font::load(document, &resources, da.font.as_bytes(), self.fonts)?;
        let style = Style::read(document, self.widget)?;
        let frame = if old.is_none() {
            Some(crate::widget_rotation::frame(style.rotation, bbox)?)
        } else {
            None
        };
        if let Some(frame) = &frame {
            bbox = frame.bounds;
        }
        let mut text = Program::new(&limits);
        let area = inset(bbox, style.inset)?;
        text.text("/Tx BMC\nq\n")?;
        text.text(&format!(
            "{} {} {} {} re W n\n",
            area.x0(),
            area.y0(),
            area.width(),
            area.height()
        ))?;
        variable::write(&mut text, &self.field, area, &font, &da)?;
        text.text("Q\nEMC\n")?;
        let text = text.finish();
        let content = if let Some(source) = source {
            update::replace(&document.stream_data(source)?, &text, &limits, self.policy)?
        } else {
            let mut out = Program::new(&limits);
            style.paint(&mut out, bbox)?;
            out.bytes(&text)?;
            out.finish()
        };
        let mut dictionary = match old {
            Some(old) if old.retained_bytes() <= limits.document_bytes => old.clone(),
            Some(_) => {
                return Err(Error::limit(
                    "appearance dictionary storage budget exceeded",
                ));
            }
            None => frame.as_ref().map_or_else(
                || form_dictionary(bbox),
                crate::widget_rotation::Frame::dictionary,
            ),
        };
        for key in [b"Length".as_slice(), b"Filter", b"DecodeParms"] {
            dictionary.remove(key);
        }
        dictionary.insert(Name::new(b"Resources"), Value::Dictionary(resources));
        AppearanceStream::new(document, dictionary, content)
    }
}

/// Generates a missing source Tx/Ch/Btn appearance through the ordinary form path.
pub(crate) fn source_widget(
    document: &Document,
    annotation: &Dictionary,
    rect: Rect,
    field: &WidgetField<'_>,
    fonts: &dyn FontSource,
) -> Result<AppearanceStream> {
    if field.has_xfa {
        return Err(Error::unsupported(
            pagura_document::features::XFA,
            "XFA appearance generation is unavailable",
        ));
    }
    Generation {
        field: variable::Inputs {
            document,
            kind: &field.kind,
            flags: field.flags,
            value: field.value.as_ref(),
            change: None,
            dictionary: field.dictionary,
            default_appearance: field.default_appearance,
            justification: field.justification,
        },
        widget: annotation,
        bounds: rect,
        default_resources: field.default_resources,
        fonts,
        policy: UpdatePolicy::Specification,
    }
    .appearance(None, field.button_state)
}

/// The stored normal appearance a widget selects with its current state,
/// before any generation, so that a reader can cull an invisible or empty
/// stored appearance before variable-text layout, as ordinary annotation
/// drawing does (12.5.5).
fn original(filler: &FormFiller<'_>, index: usize) -> Result<Option<Appearance>> {
    let widget = widget(filler, index)?;
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
        .and_then(|change| {
            change
                .widget_states()
                .iter()
                .find(|state| state.widget() == index)
        })
        .map(WidgetState::state)
        .or(widget.appearance_state)
}

fn normal(
    document: &Document,
    widget: &Widget<'_>,
    state: Option<&Name>,
) -> Result<Option<Appearance>> {
    crate::stream::stored_appearance(
        document,
        widget.dictionary,
        crate::stream::AppearanceMode::Normal,
        state,
    )
}

/// The widget's rectangle moved to the origin, the BBox of a new appearance.
fn bounds(rectangle: Rect) -> Result<Rect> {
    let (width, height) = (rectangle.width(), rectangle.height());
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

#[cfg(test)]
mod tests;
