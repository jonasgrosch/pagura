//! Confirm revision ends by parsing at Prev-chain offsets, not by searching for
//! marker text inside arbitrary document data (7.5.6; 12.8.1).
use super::RevisionCoverage;
pub(super) use crate::revision_bounds::Boundary;
use crate::revision_bounds::boundary;
use crate::source::{ReadFault, ReadResult};
use crate::view::{Context, Source};
use pagura_core::Position;

pub(super) fn read(ctx: &mut Context<'_>) -> ReadResult<Vec<Option<Boundary>>> {
    let revisions = ctx.document.revisions();
    ctx.allocate::<Option<Boundary>>(revisions.len(), Source::default())?;
    revisions
        .iter()
        .enumerate()
        .map(|(index, revision)| {
            let source = Source {
                object: None,
                position: Position::File(revision.offset()),
            };
            let end = index
                .checked_sub(1)
                .map_or(ctx.document.source_len(), |i| revisions[i].offset());
            if end <= revision.end_section {
                return Ok(None);
            }
            let result = boundary(ctx, revision, end, source);
            match result {
                Ok((boundary, consumed)) => {
                    ctx.scan(consumed, source)?;
                    Ok(Some(boundary))
                }
                Err(ReadFault::Invalid(error)) if error.kind == pagura_core::ErrorKind::Limit => {
                    Err(source.fault(error))
                }
                Err(unavailable @ ReadFault::Unavailable(_)) => Err(unavailable),
                Err(ReadFault::Invalid(_)) => Ok(None),
            }
        })
        .collect()
}

pub(super) fn coverage<'a>(
    ctx: &Context<'a>,
    boundaries: &[Option<Boundary>],
    signed_end: usize,
) -> RevisionCoverage<'a> {
    if let Some(index) = boundaries
        .iter()
        .position(|b| b.is_some_and(|b| signed_end == b.eof || signed_end == b.eol))
    {
        let revisions = ctx.document.revisions();
        RevisionCoverage::Covered {
            index,
            revision: &revisions[index],
            later_revisions: &revisions[..index],
        }
    } else if boundaries.iter().any(Option::is_none) {
        RevisionCoverage::Indeterminate
    } else {
        RevisionCoverage::NotRevisionBoundary
    }
}
