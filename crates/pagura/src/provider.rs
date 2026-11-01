//! Font and CMap resources that a PDF names but does not embed (ADR 0009).
//!
//! A [`Provider`] supplies standard-font metrics and substitute programs,
//! predefined CMaps and CID-to-Unicode tables. The default `bundled-data`
//! feature links [`Bundled`], which [`default_provider`] returns; the
//! facade's free functions ([`crate::record_page`], [`crate::text_page`],
//! [`crate::prepare_form`] …) and [`crate::render_context`] use it.
//!
//! To supply other data, implement [`Provider`] and pass it to
//! [`crate::render_context_with_provider`] or
//! [`RenderContext::with_provider`](crate::RenderContext::with_provider).
//! A provider is `Send + Sync`; share one `Arc` across documents and
//! contexts to share its parsing.

use crate::Limits;
use std::sync::Arc;

#[cfg(feature = "bundled-data")]
pub use pagura_data::Bundled;
pub use pagura_font::{FontFace, FontFormat, FontProgram};
pub use pagura_text::{
    CidToUnicode, PredefinedCMap, Provider, StandardDescriptor, StandardFont, StandardMetrics,
};

/// Whether this build links bundled font and CMap data (the `bundled-data`
/// feature), so that [`default_provider`] returns a provider.
pub const BUNDLED: bool = cfg!(feature = "bundled-data");

/// The feature-selected provider: [`Bundled`] parsing within `limits` with
/// the `bundled-data` feature, otherwise `None`, which leaves only the
/// resources embedded in each PDF. The standard fonts are parsed once per
/// process and shared by every provider; each call creates empty caches for
/// the other resources, so keep and share the result to parse each once.
///
/// ```
/// let provider = pagura::provider::default_provider(pagura::Limits::default());
/// assert_eq!(provider.is_some(), pagura::provider::BUNDLED);
/// ```
pub fn default_provider(
    #[cfg_attr(
        not(feature = "bundled-data"),
        expect(unused_variables, reason = "only bundled data parses with limits")
    )]
    limits: Limits,
) -> Option<Arc<dyn Provider>> {
    #[cfg(feature = "bundled-data")]
    return Some(Arc::new(Bundled::new(limits)));
    #[cfg(not(feature = "bundled-data"))]
    None
}
