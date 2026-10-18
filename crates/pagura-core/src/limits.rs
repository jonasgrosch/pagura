//! Per-operation resource budgets (ADR 0016).

/// Resource budgets. These bound work and memory per operation; they are
/// not an operating-system sandbox. Every budget is checked before the
/// allocation or work it protects, normally through a [`Meter`](crate::Meter).
///
/// Each field means exactly what its documentation says and is used for
/// nothing else: a parser that needs a budget no field describes gets a new
/// field (ADR 0016), never a borrowed one, so tuning one budget cannot
/// silently retune an unrelated parser. Limits defined by a standard (such
/// as a charstring stack depth) are named constants next to their parser,
/// not fields.
///
/// | Domain | Fields |
/// |---|---|
/// | Input and syntax | `file_bytes`, `depth`, `value_nodes`, `string_bytes` |
/// | Document structure | `objects`, `revisions`, `pages`, `traversal_work`, `document_bytes` |
/// | Streams | `stream_bytes`, `decoded_bytes` |
/// | Image codecs | `decode_work`, `decode_bytes` |
/// | Fonts and CMaps | `font_work`, `font_entries` |
/// | Content and rendering | `operations`, `display_commands`, `path_segments`, `raster_work`, `mask_sources`, `pixels`, `render_bytes` |
/// | Page text | `text_items`, `text_work` |
/// | Signatures | `rsa_modulus_bits`, `rsa_exponent_bits`, `rsa_work` |
///
/// Defaults target real-world viewing inside a 32-bit WebAssembly address
/// space, and every default fits a 32-bit `usize`. Callers processing
/// hostile input at scale should lower them. Construct custom limits from
/// the defaults (`Limits { pages: 10, ..Limits::default() }`), so that new
/// fields keep their defaults. Functions take `&Limits`; only a constructor
/// that stores the limits takes them by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Limits {
    /// Maximum input file size in bytes.
    pub file_bytes: usize,
    /// Aggregate estimated bytes retained by document object, object-stream and
    /// page/resource caches (ADR 0012). Excludes input bytes and temporary work.
    pub document_bytes: usize,
    /// Maximum nesting depth of arrays, dictionaries and graph traversals.
    pub depth: usize,
    /// Maximum number of cross-reference entries across all revisions.
    pub objects: usize,
    /// Maximum number of values produced by parsing one object.
    pub value_nodes: usize,
    /// Maximum decoded length of one string or name.
    pub string_bytes: usize,
    /// Maximum encoded length of one stream.
    pub stream_bytes: usize,
    /// Maximum decoded length of one stream after all filters.
    pub decoded_bytes: usize,
    /// Maximum number of revisions (incremental updates) followed.
    pub revisions: usize,
    /// Maximum number of pages in the page tree.
    pub pages: usize,
    /// Maximum number of nodes visited by one walk over a document graph:
    /// the page tree, outlines, name and number trees, the structure tree,
    /// annotation and form-field hierarchies.
    pub traversal_work: usize,
    /// Maximum number of pixels in one rendered surface.
    pub pixels: usize,
    /// Maximum number of content-stream tokens interpreted per page.
    pub operations: usize,
    /// Maximum number of line segments in one flattened path.
    pub path_segments: usize,
    /// Maximum rasterization work units (edges × scanlines touched) per fill.
    pub raster_work: usize,
    /// Maximum number of paths intersected into one clip mask.
    pub mask_sources: usize,
    /// Maximum number of painted marks in one page's display list.
    pub display_commands: usize,
    /// Maximum aggregate bytes held by the caches and surfaces of one page
    /// compilation and rendering: decoded content, images, colour
    /// transforms, fonts and compositing surfaces together.
    pub render_bytes: usize,
    /// Maximum work units for decoding one image (DCT, JPEG 2000, JBIG2,
    /// CCITT) or colour profile: roughly one unit per entropy-coded symbol,
    /// transformed coefficient or decoded sample, so the budget scales with
    /// image area rather than with a page's content.
    pub decode_work: usize,
    /// Maximum working memory one image or colour-profile decoder holds
    /// besides its output (coefficient planes, tag trees, symbol
    /// dictionaries, reference bitmaps). The output is capped by
    /// `decoded_bytes`.
    pub decode_bytes: usize,
    /// Maximum work units for parsing one font program or CMap: tokens,
    /// operators, table entries and charstring operations.
    pub font_work: usize,
    /// Maximum number of entries one font program or CMap holds: glyphs,
    /// charstrings, encoding and cmap entries, CMap code ranges and mappings.
    pub font_entries: usize,
    /// Maximum number of items recorded for one page's text: positioned
    /// glyphs, marked-content spans and text objects (ADR 0014).
    pub text_items: usize,
    /// Maximum work units for laying out, ordering, structuring and
    /// searching one page's text (ADR 0014).
    pub text_work: usize,
    /// Maximum RSA modulus bit length, also bounded by the implementation ceiling.
    pub rsa_modulus_bits: usize,
    /// Maximum RSA public exponent bit length.
    pub rsa_exponent_bits: usize,
    /// Maximum radix-2^64 limb products in one RSA verification, including
    /// Montgomery reduction. Key setup is bounded by `rsa_modulus_bits`.
    /// The caller must separately bound the number of verifications.
    pub rsa_work: usize,
}

impl Default for Limits {
    fn default() -> Self {
        const MIB: usize = 1024 * 1024;
        const PIXELS: usize = 64 * MIB;
        Self {
            file_bytes: 512 * MIB,
            document_bytes: 128 * MIB,
            depth: 64,
            objects: 4_000_000,
            value_nodes: 1_000_000,
            string_bytes: 32 * MIB,
            stream_bytes: 256 * MIB,
            decoded_bytes: 256 * MIB,
            revisions: 1024,
            pages: 200_000,
            traversal_work: 1_000_000,
            pixels: PIXELS,
            operations: 50_000_000,
            path_segments: 4_000_000,
            raster_work: 2_000_000_000,
            mask_sources: 256,
            display_commands: 10_000_000,
            render_bytes: 512 * MIB,
            // 32 units per pixel at the pixel cap: a progressive or 4:4:4
            // JPEG of that size decodes, unlike under the 50M-token
            // `operations` budget that capped codecs before (ADR 0016).
            decode_work: 32 * PIXELS,
            decode_bytes: 256 * MIB,
            font_work: 50_000_000,
            font_entries: 1_000_000,
            text_items: 1_000_000,
            text_work: 50_000_000,
            rsa_modulus_bits: 16_384,
            rsa_exponent_bits: 256,
            rsa_work: 64_000_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Limits;

    #[test]
    fn defaults_fit_a_32_bit_address_space() {
        let Limits {
            file_bytes,
            document_bytes,
            depth,
            objects,
            value_nodes,
            string_bytes,
            stream_bytes,
            decoded_bytes,
            revisions,
            pages,
            traversal_work,
            pixels,
            operations,
            path_segments,
            raster_work,
            mask_sources,
            display_commands,
            render_bytes,
            decode_work,
            decode_bytes,
            font_work,
            font_entries,
            text_items,
            text_work,
            rsa_modulus_bits,
            rsa_exponent_bits,
            rsa_work,
        } = Limits::default();
        for value in [
            file_bytes,
            document_bytes,
            depth,
            objects,
            value_nodes,
            string_bytes,
            stream_bytes,
            decoded_bytes,
            revisions,
            pages,
            traversal_work,
            pixels,
            operations,
            path_segments,
            raster_work,
            mask_sources,
            display_commands,
            render_bytes,
            decode_work,
            decode_bytes,
            font_work,
            font_entries,
            text_items,
            text_work,
            rsa_modulus_bits,
            rsa_exponent_bits,
            rsa_work,
        ] {
            assert!(u32::try_from(value).is_ok(), "{value}");
        }
    }

    #[test]
    fn decode_work_scales_with_the_pixel_budget() {
        let limits = Limits::default();
        // DCT charges 64 units per 8×8 block of each component in each scan,
        // so a 4:4:4 RGB image costs 3 units per pixel per scan. A progressive
        // one with eight scans must fit at the pixel cap.
        assert!(limits.decode_work >= 3 * 8 * limits.pixels);
        assert!(limits.decode_work > limits.operations);
    }
}
