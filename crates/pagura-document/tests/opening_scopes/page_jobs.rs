use super::*;
use pagura_document::{Page, PageJob, PageLoading, Rotation, repairs};

const BRANCH: &[u8] =
    b"2 0 obj << /Type /Pages /Count 999 /Kids [3 0 R 5 0 R] /MediaBox [0 0 8 8] >> endobj\n";
const FIRST: &[u8] = b"3 0 obj << /Type /Page /Parent 2 0 R /Resources 4 0 R >> endobj\n";
const LAST: &[u8] = b"5 0 obj << /Type /Page /Parent 2 0 R /Rotate 90 >> endobj\n";

fn fixture() -> ReadResult<(Arc<SparseBytes>, Arc<Document>)> {
    let bytes = literal();
    let source = source(&bytes)?;
    source.supply(ID, 100, ROOT)?;
    source.supply(ID, 600, BRANCH)?;
    source.supply(ID, 900, FIRST)?;
    let mut job = opening(
        source.clone(),
        Validation::RequestedOperation,
        Recovery::Repair,
    )?;
    Ok((source, ready(&mut job)?))
}

fn page(job: &mut PageJob) -> ReadResult<Page> {
    for _ in 0..32 {
        match job.advance()? {
            PageLoading::Yielded => {}
            PageLoading::Ready(page) => return Ok(page),
            _ => return Err(Error::invalid_argument("unexpected page state").into()),
        }
    }
    Err(Error::limit("test page step bound exceeded").into())
}

#[test]
fn selected_page_skips_preceding_resources_and_keeps_reported_count_unverified() {
    fn send<T: Send>() {}
    send::<PageJob>();
    let (source, document) = fixture().unwrap();
    let mut selected = PageJob::new(document.clone(), 1).unwrap();
    let Err(ReadFault::Unavailable(missing)) = page(&mut selected) else {
        panic!("second page wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(4000, 4256).unwrap()]);
    assert_eq!(selected.reported_count(), Some(999));
    assert_eq!(selected.verified_count(), None);
    let work = selected.work();
    for _ in 0..3 {
        assert_eq!(
            selected.advance().unwrap_err(),
            ReadFault::Unavailable(missing.clone())
        );
        assert_eq!(selected.work(), work);
        assert_eq!(document.diagnostics(), []);
    }
    source.supply(ID, 4000, LAST).unwrap();
    let selected_page = page(&mut selected).unwrap();
    assert_eq!(
        (selected_page.index(), selected_page.id()),
        (1, ObjectId::new(5, 0))
    );
    assert_eq!(selected_page.rotation(), Rotation::Clockwise90);
    assert_eq!(selected.verified_count(), None);
    assert!(matches!(selected.advance().unwrap(), PageLoading::Complete));
    assert!(!selected.cancel());
    assert_eq!(document.diagnostics(), []);
    assert!(matches!(
        source.read_exact(ByteRange::new(1500, 1501).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
    let mut first = PageJob::new(document.clone(), 0).unwrap();
    let Err(ReadFault::Unavailable(missing)) = page(&mut first) else {
        panic!("first page resources wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(1500, 1756).unwrap()]);
    source
        .supply(ID, 1500, b"4 0 obj << /ProcSet [/PDF] >> endobj\n")
        .unwrap();
    let first_page = page(&mut first).unwrap();
    let pages = document.pages().unwrap();
    assert_eq!(pages, [first_page, selected_page]);
    assert_eq!(
        document
            .diagnostics()
            .iter()
            .map(pagura_core::Diagnostic::repair)
            .collect::<Vec<_>>(),
        [Some(repairs::PAGE_COUNT)]
    );
}

#[test]
fn first_page_completes_without_later_subtrees_and_cancellation_drops_demand() {
    let (source, document) = fixture().unwrap();
    source.supply(ID, 1500, b"4 0 obj << >> endobj\n").unwrap();
    let mut first = PageJob::new(document.clone(), 0).unwrap();
    let ready = page(&mut first).unwrap();
    assert_eq!(ready.id(), ObjectId::new(3, 0));
    assert_eq!(ready.index(), 0);
    assert_eq!(first.reported_count(), Some(999));
    assert_eq!(first.verified_count(), None);
    assert!(matches!(
        source.read_exact(ByteRange::new(4000, 4001).unwrap()),
        Err(ReadFault::Unavailable(_))
    ));
    let mut second = PageJob::new(document.clone(), 1).unwrap();
    assert!(matches!(page(&mut second), Err(ReadFault::Unavailable(_))));
    let work = second.work();
    assert!(second.cancel());
    assert!(!second.cancel());
    source.supply(ID, 4000, LAST).unwrap();
    assert!(matches!(second.advance().unwrap(), PageLoading::Cancelled));
    assert_eq!(second.work(), work);
    source.close();
    assert_eq!(ready.media_box().width(), 8.0);
    assert!(matches!(first.advance().unwrap(), PageLoading::Complete));
    assert!(PageJob::new(document, 0).is_err());
}

#[test]
fn out_of_bounds_uses_a_verified_total_and_terminal_errors_do_not_resume() {
    let (source, document) = fixture().unwrap();
    source.supply(ID, 4000, LAST).unwrap();
    let mut missing = PageJob::new(document.clone(), 2).unwrap();
    let error = page(&mut missing).unwrap_err();
    assert_eq!(error.as_error().unwrap().kind, ErrorKind::InvalidArgument);
    assert_eq!(missing.reported_count(), Some(999));
    assert_eq!(missing.verified_count(), Some(2));
    assert_eq!(
        document
            .diagnostics()
            .iter()
            .map(pagura_core::Diagnostic::repair)
            .collect::<Vec<_>>(),
        [Some(repairs::PAGE_COUNT)]
    );
    let work = missing.work();
    for _ in 0..3 {
        assert_eq!(missing.advance().unwrap_err(), error);
    }
    assert_eq!(missing.work(), work);
    assert!(!missing.cancel());
    let mut closed = PageJob::new(document, 0).unwrap();
    assert!(matches!(page(&mut closed), Err(ReadFault::Unavailable(_))));
    source.close();
    let error = source.check().unwrap_err();
    assert_eq!(
        closed.advance().unwrap_err(),
        ReadFault::Invalid(error.clone())
    );
    assert_eq!(closed.advance().unwrap_err(), ReadFault::Invalid(error));
}
