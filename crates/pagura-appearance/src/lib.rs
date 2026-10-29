#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
//! Form-field and annotation appearance generation (12.5.5, 12.7.4.3): from
//! field values, default appearances and annotation properties to
//! appearance content and resources, which the interpreter draws like any
//! appearance stream. It knows nothing about interpretation or pixels.
//!
//! - Form sessions: [`prepare`], [`prepare_widget`] and
//!   [`prepare_widget_with_policy`] select or generate each widget's normal
//!   appearance for the in-memory values of a
//!   [`FormFiller`](pagura_document::forms::FormFiller); an interpreter
//!   drawing a page with those values asks [`FilledWidgets`] per widget
//!   annotation.
//! - Annotations without an appearance stream: [`annotation_appearance`].
//! - Fonts: variable text is laid out with fonts from a [`FontSource`],
//!   such as [`ProviderFonts`] or an interpreter's font cache.
//!
//! Every generated appearance is an [`AppearanceStream`]: a complete Form
//! XObject dictionary (BBox, Resources and, for an updated stream, its other
//! entries) and decoded content, never written into the document.

mod annotation;
pub mod features;
mod fonts;
mod form;
pub mod information;
mod program;
mod stream;
mod values;

pub use annotation::annotation_appearance;
pub use fonts::{FontSource, ProviderFonts};
pub use form::{
    FilledWidgets, TextStyle, UpdatePolicy, WidgetAppearance, default_text_style, prepare,
    prepare_widget, prepare_widget_with_policy,
};
pub use stream::{Appearance, AppearanceStream};
