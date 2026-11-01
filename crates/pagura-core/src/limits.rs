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
/// not fields; [`Limits::CALCULATOR_STACK`] is the one shared here.
///
/// | Domain | Fields |
/// |---|---|
/// | Input and syntax | `file_bytes`, `depth`, `value_nodes`, `string_bytes` |
/// | Document structure | `objects`, `revisions`, `pages`, `traversal_work`, `document_bytes` |
/// | Streams | `stream_bytes`, `decoded_bytes` |
/// | Image codecs | `decode_work`, `decode_bytes` |
/// | Tiled image collections | `tiled_images`: [`TiledImageLimits`] |
/// | Fonts and CMaps | `font_work`, `font_entries` |
/// | Content and rendering | `operations`, `display_commands`, `path_segments`, `raster_work`, `mask_sources`, `pixels`, `render_bytes`, `render_work` |
/// | Page text | `text_items`, `text_work` |
/// | Functions | `function_work` |
/// | Signatures | `signatures`: [`SignatureLimits`] for ASN.1, certification paths and RSA |
///
/// Defaults target real-world viewing inside a 32-bit WebAssembly address
/// space, and every default fits a 32-bit `usize`. Callers processing
/// hostile input at scale should lower them. Construct custom limits from
/// the defaults (`Limits { pages: 10, ..Limits::default() }`), so that new
/// fields keep their defaults. Functions take `&Limits`; only a constructor
/// that stores the limits takes them by value.
///
/// The struct stays within 264 bytes on 64-bit targets and 156 bytes on
/// wasm32. Domain groups use `u32` fields rather than growing every budget
/// to a native address width (ADR 0016). Non-storing functions borrow limits;
/// constructors retaining them keep ownership. Unit tests enforce these bounds.
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
    /// annotation and form-field hierarchies, and the signature fields,
    /// signed revisions and DocMDP/FieldMDP change analysis of signature
    /// inspection (12.8).
    pub traversal_work: usize,
    /// Maximum width × height of one declared rendered or decoded surface.
    /// Storage layout does not change this bound. An explicit collection of
    /// bounded surfaces additionally obeys `tiled_images` aggregate limits.
    pub pixels: usize,
    /// Maximum number of content-stream tokens interpreted per page.
    pub operations: usize,
    /// Maximum number of line segments in one flattened path.
    pub path_segments: usize,
    /// Maximum rasterization work units (edges × scanlines touched) per fill.
    /// A render's cumulative work is `render_work`.
    pub raster_work: usize,
    /// Maximum number of paths intersected into one clip mask.
    pub mask_sources: usize,
    /// Maximum number of painted marks in one page's display list.
    pub display_commands: usize,
    /// Maximum bytes one render holds besides the values it takes from the
    /// [`Store`](crate::Store): prepared marks, coverage sources and paint,
    /// the band buffers of the page, groups and soft masks, coverage rows
    /// and scratch. Reserved during preparation (ADR 0016, ADR 0034).
    pub render_bytes: usize,
    /// Maximum work units of one render (ADR 0016, ADR 0034): the
    /// rasterization work of the marks that meet the region, plus one unit
    /// per pixel each of them composites. Reserved during preparation, so
    /// execution cannot exceed it.
    pub render_work: usize,
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
    /// Maximum work units for evaluating functions (7.10) during one render:
    /// one unit per Type 4 calculator operator executed and per sample
    /// looked up or interpolated in a sampled, exponential or stitching
    /// function. Reserved during preparation through the prepared sources
    /// that evaluate them.
    pub function_work: usize,
    /// Aggregate budgets of one explicit tiled image collection (ADR 0010).
    pub tiled_images: TiledImageLimits,
    /// Budgets of signature validation (ADR 0015).
    pub signatures: SignatureLimits,
}

// These are reviewed record-layout bounds, not a Clippy threshold override.
// Compile-time checks cover wasm builds that do not execute Rust unit tests.
#[cfg(target_pointer_width = "64")]
const _: [(); 264] = [(); size_of::<Limits>()];
#[cfg(target_pointer_width = "32")]
const _: [(); 156] = [(); size_of::<Limits>()];

/// Aggregate budgets of one explicit tiled image collection.
///
/// Every field is `u32`, including on native targets. The record supports
/// functional update syntax so callers can lower one domain independently.
/// Every tile still obeys [`Limits::pixels`], and the whole collection shares
/// [`Limits::decoded_bytes`] output and [`Limits::decode_bytes`] scratch caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TiledImageLimits {
    /// Maximum logical component sample codes in the complete collection,
    /// summed once over its tiles; never a fresh allowance per tile.
    pub samples: u32,
    /// Maximum work constructing the complete collection: one unit per
    /// sample-code read/write, packed byte read/write/initialization and
    /// processed tile or row record. Filter entropy work is not included.
    pub work: u32,
}

impl Default for TiledImageLimits {
    fn default() -> Self {
        Self {
            samples: 256 * 1024 * 1024,
            work: 512 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// Operand-stack entries of a Type 4 (PostScript calculator) function
    /// (7.10.5), the architectural limit of ISO 32000-1 Annex C, Table C.1.
    /// A standard limit, not a budget, so it is not a field.
    pub const CALCULATOR_STACK: usize = 100;
}

/// Budgets of signature validation (ADR 0015): ASN.1 encodings,
/// certification paths (RFC 5280 6) and RSA verification. The fields are
/// `u32`, which every default fits, so that [`Limits`] stays small; widen
/// them with [`u32_to_usize`](crate::convert::u32_to_usize).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignatureLimits {
    /// Maximum nesting depth of one ASN.1 encoding (X.690 8.1): a signature's
    /// CMS, a certificate, a CRL or a timestamp.
    pub asn1_depth: u32,
    /// Maximum number of elements indexed for one ASN.1 encoding. A component
    /// that is checked again as DER is indexed again under the same limit.
    pub asn1_elements: u32,
    /// Maximum bytes one ASN.1 operation allocates besides its input: the
    /// element index and traversal stack of one encoding, or one value copied
    /// out of it (concatenated BER OCTET STRING content, the DER signature
    /// input of signed attributes).
    pub asn1_bytes: u32,
    /// Maximum number of certificates in one certification path, excluding
    /// the trust anchor (RFC 5280 6.1).
    pub path_certificates: u32,
    /// Maximum work units for one certification path validation or search,
    /// including its CRL processing: scanned bytes, name and OID comparisons,
    /// policy-graph operations and the bytes of each signature input.
    pub path_work: u32,
    /// Maximum values one path validation or search prepares: certificates,
    /// extension and reparsed DER elements, queued branches, result paths and
    /// policy-graph nodes and edges.
    pub path_nodes: u32,
    /// Maximum estimated bytes one path validation or search allocates: copied
    /// names, OIDs and policy storage, temporary extension indexes and results.
    pub path_bytes: u32,
    /// Maximum distinct signature verifications one path validation or search
    /// asks the host to perform; repeated inputs are answered from memory.
    pub path_verifications: u32,
    /// Maximum RSA modulus bit length, also bounded by the implementation
    /// ceiling.
    pub rsa_modulus_bits: u32,
    /// Maximum RSA public exponent bit length.
    pub rsa_exponent_bits: u32,
    /// Maximum radix-2^64 limb products in one RSA verification, including
    /// Montgomery reduction. Key setup is bounded by `rsa_modulus_bits`.
    /// The caller must separately bound the number of verifications.
    pub rsa_work: u32,
}

impl Default for SignatureLimits {
    fn default() -> Self {
        const MIB: u32 = 1024 * 1024;
        Self {
            asn1_depth: 64,
            asn1_elements: 1_000_000,
            asn1_bytes: 128 * MIB,
            path_certificates: 64,
            path_work: 50_000_000,
            path_nodes: 1_000_000,
            path_bytes: 128 * MIB,
            path_verifications: 100,
            rsa_modulus_bits: 16_384,
            rsa_exponent_bits: 256,
            rsa_work: 64_000_000,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        const MIB: usize = 1024 * 1024;
        const PIXELS: usize = 64 * MIB;
        const WORK: usize = 32 * PIXELS;
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
            render_work: 2_000_000_000,
            // 32 units per pixel at the pixel cap: a progressive or 4:4:4
            // JPEG of that size decodes, unlike under the 50M-token
            // `operations` budget that capped codecs before (ADR 0016).
            decode_work: WORK,
            decode_bytes: 256 * MIB,
            font_work: 50_000_000,
            font_entries: 1_000_000,
            text_items: 1_000_000,
            text_work: 50_000_000,
            function_work: WORK,
            tiled_images: TiledImageLimits::default(),
            signatures: SignatureLimits::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Limits, SignatureLimits, TiledImageLimits};

    #[test]
    fn defaults_fit_a_32_bit_address_space() {
        // Destructured exhaustively so that a new field is checked too; the
        // signature group is u32 by type.
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
            render_work,
            decode_work,
            decode_bytes,
            font_work,
            font_entries,
            text_items,
            text_work,
            function_work,
            tiled_images: TiledImageLimits { samples, work },
            signatures: _,
        } = Limits::default();
        assert!(usize::try_from(samples).is_ok());
        assert!(usize::try_from(work).is_ok());
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
            render_work,
            decode_work,
            decode_bytes,
            font_work,
            font_entries,
            text_items,
            text_work,
            function_work,
        ] {
            assert!(u32::try_from(value).is_ok(), "{value}");
        }
    }

    #[test]
    fn limits_record_size_is_bounded_on_native_and_wasm32() {
        // An explicit eight-byte domain expansion, without a lint threshold
        // override or narrowing any existing custom usize budget.
        let maximum = if size_of::<usize>() == 8 { 264 } else { 156 };
        assert_eq!(size_of::<Limits>(), maximum);
        assert_eq!(size_of::<TiledImageLimits>(), 8);
        assert_eq!(Limits::default().tiled_images, TiledImageLimits::default());
        assert_eq!(Limits::default().signatures, SignatureLimits::default());
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
