//! Session budgets, separate from the engine's per-document `Limits` (ADR 0032).

use crate::protocol::layout;
use pagura::Limits;

const MIB: usize = 1024 * 1024;

/// Budgets of one session as a whole. Each field means only what it says;
/// none is borrowed from [`Limits`], which every open document receives
/// unchanged.
///
/// Like `Limits`, this is built by functional record update
/// (`SessionLimits { max_documents: 4, ..SessionLimits::default() }`), so it
/// is not `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLimits {
    /// Documents open at the same time. Handles are never reused, so this
    /// does not limit how many documents a session opens over its lifetime.
    pub max_documents: usize,
    /// Original source bytes retained across all open documents.
    pub total_source_bytes: usize,
    /// Capacity of each open document's render store (ADR 0012): the
    /// recorded scenes, decoded images, fonts and glyph setups rendering
    /// retains between tiles and zoom levels. Zero disables retention.
    pub store_bytes: usize,
    /// Accounting budget of the extracted-text cache; zero disables retention.
    pub text_cache_bytes: usize,
    /// Payload bytes of one reading response (protocol 1.2 and later).
    pub reading_response_bytes: usize,
    /// Body bytes of any frame, in either direction, checked before a
    /// request body is read. At most `u32::MAX`, the wire's length field.
    pub max_frame_bytes: usize,
}

impl SessionLimits {
    /// Default [`SessionLimits::max_documents`].
    pub const DEFAULT_MAX_DOCUMENTS: usize = 256;
    /// Default [`SessionLimits::total_source_bytes`]: one maximum-size file
    /// under `Limits::default()`.
    pub const DEFAULT_TOTAL_SOURCE_BYTES: usize = 512 * MIB;
    /// Default [`SessionLimits::store_bytes`].
    pub const DEFAULT_STORE_BYTES: usize = 64 * MIB;
    /// Default [`SessionLimits::text_cache_bytes`].
    pub const DEFAULT_TEXT_CACHE_BYTES: usize = 16 * MIB;
    /// Default [`SessionLimits::reading_response_bytes`].
    pub const DEFAULT_READING_RESPONSE_BYTES: usize = 32 * MIB;
    /// The smallest frame cap, which leaves room for a structured error
    /// even when every engine budget is zero.
    pub const MIN_FRAME_BYTES: usize = 1024;

    /// The default session budgets, with a frame cap that fits the largest
    /// request and response `engine` allows: an open of a maximum-size file
    /// and password, a maximum-size generated tile, and a full reading
    /// response.
    pub const fn for_engine(engine: &Limits) -> Self {
        let mut limits = Self {
            max_documents: Self::DEFAULT_MAX_DOCUMENTS,
            total_source_bytes: Self::DEFAULT_TOTAL_SOURCE_BYTES,
            store_bytes: Self::DEFAULT_STORE_BYTES,
            text_cache_bytes: Self::DEFAULT_TEXT_CACHE_BYTES,
            reading_response_bytes: Self::DEFAULT_READING_RESPONSE_BYTES,
            max_frame_bytes: 0,
        };
        limits.max_frame_bytes = limits.frame_bytes_for(engine);
        limits
    }

    /// The frame cap [`SessionLimits::for_engine`] derives: the largest of
    /// `file_bytes + string_bytes + 14` (open), `pixels * 4 + 18` (generated
    /// tile), `reading_response_bytes + 5` and [`Self::MIN_FRAME_BYTES`],
    /// with saturating arithmetic, at most `u32::MAX`.
    pub const fn frame_bytes_for(&self, engine: &Limits) -> usize {
        let open = engine
            .file_bytes
            .saturating_add(engine.string_bytes)
            .saturating_add(layout::OPEN_OVERHEAD);
        let tile = engine
            .pixels
            .saturating_mul(4)
            .saturating_add(layout::GENERATION_TILE_OVERHEAD);
        let reading = self
            .reading_response_bytes
            .saturating_add(layout::HEADER_BYTES);
        let bytes = max(max(open, tile), max(reading, Self::MIN_FRAME_BYTES));
        if bytes > layout::MAX_BODY_BYTES {
            layout::MAX_BODY_BYTES
        } else {
            bytes
        }
    }
}

const fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self::for_engine(&Limits::default())
    }
}

/// Everything a [`crate::Session`] is configured with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionConfig {
    /// Budgets each open document receives (ADR 0016).
    pub engine: Limits,
    /// Budgets of the session as a whole.
    pub session: SessionLimits,
}

impl SessionConfig {
    /// `engine` budgets with [`SessionLimits::for_engine`].
    pub const fn new(engine: Limits) -> Self {
        Self {
            engine,
            session: SessionLimits::for_engine(&engine),
        }
    }

    /// Replaces the session budgets.
    #[must_use]
    pub const fn with_session(mut self, session: SessionLimits) -> Self {
        self.session = session;
        self
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl From<Limits> for SessionConfig {
    fn from(engine: Limits) -> Self {
        Self::new(engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_frame_cap_fits_the_largest_open_and_is_independent_of_decoded_bytes() {
        // 512 MiB file + 32 MiB password + 14 header bytes, as web/protocol.mjs
        // hard-coded before it imported the generated constant.
        let defaults = SessionLimits::default();
        assert_eq!(defaults.max_frame_bytes, 512 * MIB + 32 * MIB + 14);
        let engine = Limits {
            decoded_bytes: usize::MAX,
            ..Limits::default()
        };
        assert_eq!(SessionLimits::for_engine(&engine), defaults);
    }

    #[test]
    fn frame_cap_saturates_and_keeps_room_for_errors() {
        let zero = Limits {
            file_bytes: 0,
            string_bytes: 0,
            pixels: 0,
            ..Limits::default()
        };
        let session = SessionLimits {
            reading_response_bytes: 0,
            ..SessionLimits::default()
        };
        assert_eq!(
            session.frame_bytes_for(&zero),
            SessionLimits::MIN_FRAME_BYTES
        );
        let huge = Limits {
            file_bytes: usize::MAX,
            ..Limits::default()
        };
        assert_eq!(
            SessionLimits::for_engine(&huge).max_frame_bytes,
            layout::MAX_BODY_BYTES
        );
        let tiles = Limits {
            pixels: 1 << 28,
            ..zero
        };
        assert_eq!(session.frame_bytes_for(&tiles), (1 << 30) + 18);
    }
}
