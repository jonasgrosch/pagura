//! Confirm revision ends at Prev-chain offsets, not by searching for marker
//! text inside arbitrary document data (7.5.6; 12.8.1). Opening already parsed
//! each revision's cross-reference section; only its `startxref` and `%%EOF`
//! tail is read here.

use super::RevisionCoverage;
use crate::Revision;
pub(super) use crate::revision_bounds::Boundary;
use crate::revision_bounds::boundary;
use crate::view::{Context, Source};
use pagura_core::{ErrorKind, Position, Result};

pub(super) fn read(ctx: &mut Context<'_>) -> Result<Vec<Option<Boundary>>> {
    let revisions = ctx.document.revisions();
    ctx.allocate::<Option<Boundary>>(revisions.len(), Source::default())?;
    let file = ctx.document.original_bytes().len();
    let mut boundaries = Vec::with_capacity(revisions.len());
    for (index, revision) in revisions.iter().enumerate() {
        let source = Source {
            object: None,
            position: Position::File(revision.offset()),
        };
        // A revision ends before the next one starts.
        let end = index
            .checked_sub(1)
            .and_then(|newer| revisions.get(newer))
            .map_or(file, Revision::offset);
        if revision.end > end {
            boundaries.push(None);
            continue;
        }
        // One linear pass over this revision's tail. The tails of different
        // revisions are disjoint, so the work is bounded by the file itself.
        boundaries.push(match boundary(ctx, revision, end, source) {
            Ok(boundary) => {
                ctx.scan(boundary.eol - revision.end, source)?;
                Some(boundary)
            }
            Err(error) if error.kind == ErrorKind::Limit => return Err(source.error(error)),
            Err(_) => None,
        });
    }
    Ok(boundaries)
}

pub(super) fn coverage<'a>(
    ctx: &Context<'a>,
    boundaries: &[Option<Boundary>],
    signed_end: usize,
) -> RevisionCoverage<'a> {
    let revisions = ctx.document.revisions();
    if let Some(index) = boundaries.iter().position(|boundary| {
        boundary.is_some_and(|boundary| signed_end == boundary.eof || signed_end == boundary.eol)
    }) && let (Some(revision), Some(later_revisions)) =
        (revisions.get(index), revisions.get(..index))
    {
        RevisionCoverage::Covered {
            index,
            revision,
            later_revisions,
        }
    } else if boundaries.iter().any(Option::is_none) {
        RevisionCoverage::Indeterminate
    } else {
        RevisionCoverage::NotRevisionBoundary
    }
}
