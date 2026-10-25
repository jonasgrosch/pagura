//! Helper threads render the bytes of one thread and count work once; they
//! take only the render budget preparation left (ADR 0035).

use super::invariance::busy;
use crate::{Completion, Scratch};
use pagura_core::Limits;

#[test]
fn helper_threads_render_the_bytes_of_one_thread() {
    let b = busy();
    let page = b.page();
    for height in [1, 5, 8, 32] {
        let request = b.request(page, height);
        let prepared = b
            .prepare(&request, &Limits::default())
            .unwrap_or_else(|e| panic!("{e}"));
        let mut scratch = Scratch::default();
        let mut alone = prepared
            .with_scratch(&mut scratch)
            .unwrap_or_else(|e| panic!("{e}"));
        let mut expected = Vec::new();
        alone.render(&mut expected);
        let work = alone.workspace.usage.work;
        let mut scratches: Vec<Scratch> = (0..4).map(|_| Scratch::default()).collect();
        for threads in 1..=scratches.len() {
            let mut rendering = prepared
                .with_scratches(&mut scratches[..threads])
                .unwrap_or_else(|e| panic!("{e}"));
            let bands = prepared.plan.area.bands().len();
            assert_eq!(
                rendering.helpers.len(),
                (threads - 1).min(bands - 1),
                "height {height}"
            );
            // Twice, so that reused storage is checked too.
            for _ in 0..2 {
                let mut rows = Vec::new();
                rendering.render(&mut rows);
                assert_eq!(rows, expected, "height {height}, {threads} threads");
                let shared = rendering.workspace.usage.work
                    + rendering
                        .helpers
                        .iter()
                        .map(|helper| helper.workspace.usage.work)
                        .sum::<usize>();
                assert_eq!(shared, work, "height {height}, {threads} threads");
            }
        }
    }
}

#[test]
fn helper_threads_stop_in_band_order() {
    let b = busy();
    let page = b.page();
    let prepared = b
        .prepare(&b.request(page, 8), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let whole = b
        .render(&b.request(page, 8))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut scratches: Vec<Scratch> = (0..3).map(|_| Scratch::default()).collect();
    let mut rendering = prepared
        .with_scratches(&mut scratches)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut asked = Vec::new();
    let mut rows = Vec::new();
    let completion = rendering.render_until(&mut rows, |row| {
        asked.push(row);
        row >= 16
    });
    assert_eq!(completion, Completion::Stopped { row: 16 });
    assert_eq!(asked, [0, 8, 16]);
    let bytes = 16 * page.width() as usize * 4;
    assert_eq!(rows, whole[..bytes]);
}

#[test]
fn helpers_take_only_the_render_budget_left() {
    let b = busy();
    let request = b.request(b.page(), 8);
    let free = b
        .prepare(&request, &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let charges = free.charges();
    let scratch = free.plan.scratch;
    let mut scratches: Vec<Scratch> = (0..4).map(|_| Scratch::default()).collect();
    for (room, helpers) in [(0, 0), (scratch - 1, 0), (2 * scratch, 2), (9 * scratch, 3)] {
        let limits = Limits {
            render_bytes: charges.render_bytes() + room,
            ..Limits::default()
        };
        let prepared = b
            .prepare(&request, &limits)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(prepared.charges(), charges);
        let rendering = prepared
            .with_scratches(&mut scratches)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(rendering.helpers.len(), helpers, "room {room}");
    }
    // One band leaves nothing to help with.
    let single = b
        .prepare(&b.request(b.page(), 256), &Limits::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let rendering = single
        .with_scratches(&mut scratches)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(rendering.helpers.is_empty());
}
