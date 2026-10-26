use super::*;
use crate::source::{
    ByteRange, ByteSource, LoadingLimits, SourceIdentity, SourceLimits, SparseBytes,
};
use crate::{OpenJob, OpenOptions, Opening, Validation};
use pagura_core::{Limits, Position};

const ID: SourceIdentity = SourceIdentity::new(829, [37; 32]);
const SECOND: &[u8] = b"4 0 obj << /Type /Page /Parent 2 0 R /Rotate 6 0 R >> endobj\n";

fn fixture(pages: &[u8]) -> ReadResult<(Arc<SparseBytes>, Arc<Document>)> {
    let source = Arc::new(SparseBytes::new(
        ID,
        8192,
        &Limits::default(),
        &SourceLimits::default(),
    )?);
    for (offset, bytes) in [
        (0, b"%PDF-1.7\n".as_slice()),
        (100, b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n".as_slice()),
        (600, pages),
        (900, b"3 0 obj << /Parent 2 0 R /MediaBox [0 0 4 4] >> endobj\n".as_slice()),
        (6500, b"xref\n0 5\n0 65535 f\n100 0 n\n600 0 n\n900 0 n\n4000 0 n\n6 1\n6000 0 n\ntrailer << /Size 7 /Root 1 0 R >>\n".as_slice()),
    ] { source.supply(ID, offset, bytes)?; }
    let mut tail = vec![b' '; 1024];
    tail[932..953].copy_from_slice(b"startxref\n6500\n%%EOF\n");
    source.supply(ID, 7168, &tail)?;
    let mut job = OpenJob::new(
        source.clone(),
        &OpenOptions::default(),
        LoadingLimits::default(),
        Validation::RequestedOperation,
    )?;
    for _ in 0..16 {
        match job.advance()? {
            Opening::Yielded => {}
            Opening::Ready(document) => return Ok((source, document)),
            _ => return Err(Error::invalid_argument("unexpected opening state").into()),
        }
    }
    Err(Error::limit("test opening step bound exceeded").into())
}

fn run(cursor: &mut Cursor, document: &Document) -> ReadResult<()> {
    for _ in 0..64 {
        if cursor.advance(document)? {
            return Ok(());
        }
    }
    Err(Error::limit("test traversal step bound exceeded").into())
}

#[test]
fn owned_cursor_retains_completed_occurrences_and_inheritance_across_waits() {
    fn send<T: Send>() {}
    send::<Cursor>();
    let (source, document) = fixture(b"2 0 obj << /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 3 0 R] /MediaBox [0 0 8 8] /Resources << /ProcSet [/PDF] >> >> endobj\n").unwrap();
    let mut cursor = Cursor::new(&document, Selection::All).unwrap();
    let Err(ReadFault::Unavailable(missing)) = run(&mut cursor, &document) else {
        panic!("second page wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(4000, 4256).unwrap()]);
    assert_eq!(cursor.pages.len(), 1);
    let first_resources = Arc::clone(&cursor.pages[0].resources);
    let retained = document.retained.used();
    let work = cursor.work.used();
    for _ in 0..3 {
        assert_eq!(
            cursor.advance(&document).unwrap_err(),
            ReadFault::Unavailable(missing.clone())
        );
        assert_eq!(document.retained.used(), retained);
        assert_eq!(cursor.work.used(), work);
        assert_eq!(document.diagnostics(), []);
    }
    source.supply(ID, 4000, SECOND).unwrap();
    let Err(ReadFault::Unavailable(missing)) = run(&mut cursor, &document) else {
        panic!("rotation wait")
    };
    assert_eq!(missing.ranges(), &[ByteRange::new(6000, 6256).unwrap()]);
    assert_eq!(cursor.pages.len(), 1);
    assert!(Arc::ptr_eq(&first_resources, &cursor.pages[0].resources));
    assert_eq!(document.diagnostics(), []);
    source.supply(ID, 6000, b"6 0 obj 90 endobj\n").unwrap();
    run(&mut cursor, &document).unwrap();
    let finished = cursor.finish().unwrap();
    assert_eq!(finished.pages.len(), 3);
    assert_eq!(
        finished.pages.iter().map(Page::index).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(finished.pages[0].id(), ObjectId::new(3, 0));
    assert_eq!(finished.pages[1].id(), ObjectId::new(4, 0));
    assert_eq!(finished.pages[2].id(), ObjectId::new(3, 0));
    assert_eq!(
        finished.pages[0].media_box(),
        pagura_core::Rect::from_corners(0.0, 0.0, 4.0, 4.0).unwrap()
    );
    assert_eq!(
        finished.pages[1].rotation(),
        Rotation::try_from(90).unwrap()
    );
    for page in &finished.pages {
        assert!(Arc::ptr_eq(&first_resources, &page.resources));
    }
    assert_eq!(
        finished
            .diagnostics
            .iter()
            .map(Diagnostic::repair)
            .collect::<Vec<_>>(),
        [
            Some(repairs::PAGE_TYPE),
            Some(repairs::PAGE_REPEAT),
            Some(repairs::PAGE_TYPE)
        ]
    );
    assert_eq!(finished.diagnostics[0].position, Position::File(900));
    assert_eq!(document.diagnostics(), []);
    let expected = finished.pages;
    drop(finished.reservation);
    assert_eq!(document.pages().unwrap(), expected);
    assert_eq!(
        document
            .diagnostics()
            .iter()
            .map(Diagnostic::repair)
            .collect::<Vec<_>>(),
        [Some(repairs::PAGE_TYPE), Some(repairs::PAGE_REPEAT)]
    );
}

#[test]
fn dropping_a_waiting_cursor_releases_only_its_pages_and_resources() {
    let (source, document) = fixture(
        b"2 0 obj << /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] /MediaBox [0 0 8 8] >> endobj\n",
    )
    .unwrap();
    document.object(ObjectId::new(2, 0)).unwrap();
    document.object(ObjectId::new(3, 0)).unwrap();
    let baseline = document.retained.used();
    let mut cursor = Cursor::new(&document, Selection::All).unwrap();
    assert!(matches!(
        run(&mut cursor, &document),
        Err(ReadFault::Unavailable(_))
    ));
    assert_eq!(cursor.pages.len(), 1);
    assert!(document.retained.used() > baseline);
    source.close();
    assert_eq!(
        cursor.advance(&document).unwrap_err(),
        source.check().unwrap_err().into()
    );
    drop(cursor);
    assert_eq!(document.retained.used(), baseline);
    assert_eq!(document.diagnostics(), []);
}

#[test]
fn selected_page_reservation_follows_the_issued_value_and_its_clones() {
    use crate::{PageJob, PageLoading};
    let (_, document) = fixture(
        b"2 0 obj << /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] /MediaBox [0 0 8 8] >> endobj\n",
    )
    .unwrap();
    document.object(ObjectId::new(2, 0)).unwrap();
    document.object(ObjectId::new(3, 0)).unwrap();
    let baseline = document.retained.used();
    let mut job = PageJob::new(document.clone(), 0).unwrap();
    let mut result = None;
    for _ in 0..8 {
        match job.advance().unwrap() {
            PageLoading::Yielded => {}
            PageLoading::Ready(page) => {
                result = Some(page);
                break;
            }
            other => panic!("unexpected selected page state {other:?}"),
        }
    }
    let page = result.unwrap();
    let expected = size_of::<Page>() + page.dictionary.retained_bytes() - size_of::<Dictionary>()
        + page.resources.retained_bytes()
        + ARC_COUNTS
        + size_of::<Reservation>()
        + ARC_COUNTS;
    assert_eq!(document.retained.used(), baseline + expected);
    drop(job);
    assert_eq!(document.retained.used(), baseline + expected);
    let copy = page.clone();
    drop(page);
    assert_eq!(document.retained.used(), baseline + expected);
    assert_eq!(copy.index(), 0);
    drop(copy);
    assert_eq!(document.retained.used(), baseline);
}
