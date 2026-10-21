//! Diagnostic of the actual Segment vector growth and mem::take retention.
//! It models the compiler's five pushes; it does not invoke the PDF renderer.
use pagura_raster::Segment;

fn main() {
    for (case, rectangles, extra) in [("single", 1, 0), ("rectangle_then_line", 1, 1), ("three_rectangles", 3, 0)] {
        let mut old = Vec::<Segment>::new();
        let mut new = Vec::<Segment>::new();
        for _ in 0..rectangles {
            new.reserve(5);
            for _ in 0..5 {
                old.push(Segment::Close);
                new.push(Segment::Close);
            }
        }
        for _ in 0..extra {
            old.push(Segment::Close);
            new.push(Segment::Close);
        }
        let old_path = std::mem::take(&mut old);
        let new_path = std::mem::take(&mut new);
        println!("{{\"case\":\"{}\",\"length\":{},\"segment_bytes\":{},\"baseline_retained_capacity\":{},\"candidate_retained_capacity\":{},\"baseline_builder_capacity\":{},\"candidate_builder_capacity\":{}}}",
            case, old_path.len(), std::mem::size_of::<Segment>(), old_path.capacity(), new_path.capacity(), old.capacity(), new.capacity());
    }
}
