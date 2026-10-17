//! Per-operation resource budgets.

/// Resource budgets. These bound work and memory per operation; they are
/// not an operating-system sandbox. Every budget is checked before the
/// allocation or work it protects.
///
/// Defaults target real-world viewing inside a 32-bit WebAssembly address
/// space. Callers processing hostile input at scale should lower them.
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
}

impl Default for Limits {
    fn default() -> Self {
        const MIB: usize = 1024 * 1024;
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
            pixels: 64 * MIB,
            operations: 50_000_000,
            path_segments: 4_000_000,
            raster_work: 2_000_000_000,
            mask_sources: 256,
            display_commands: 10_000_000,
        }
    }
}
