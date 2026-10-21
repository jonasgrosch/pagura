//! Hand-built scenes through preparation and execution, against the
//! reference renderer and closed forms.

use crate::testing::{Build, assert_within_one_level, mark};
use pagura_interpret::{GroupId, StateId};
use pagura_raster::Rgba8;

#[test]
fn opaque_rectangles_render_within_one_level() {
    let mut b = Build::new(8.0, 6.0);
    let red = b.rgb([1.0, 0.0, 0.0]);
    let green = b.rgb([0.0, 0.6, 0.2]);
    b.rect(GroupId(0), [0.25, 0.5, 5.0, 4.75], red, mark(StateId(0)));
    b.rect(GroupId(0), [2.5, 1.25, 7.5, 5.5], green, mark(StateId(0)));
    let bytes = b.render_page();
    assert_within_one_level(&bytes, &b.reference(Rgba8::WHITE));
}
