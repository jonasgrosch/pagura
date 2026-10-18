//! Which scenes the elided-group and 8-bit paths may take (11.4.5, 11.4.6).

use crate::display::{Command, Mark, PathCommand};
use crate::transparency::{Group, Model};

pub(super) fn opaque_knockout(group: &Group) -> bool {
    group.attributes.view.is_none()
        && !group.attributes.isolated
        && group.attributes.knockout
        && group.attributes.model.is_none_or(|m| m == Model::Rgb)
        && group.composite.simple()
        && group.commands.iter().all(|c| {
            let p = match c {
                Command::Path(p) => p,
                Command::Glyphs(run) => &run.paint,
                _ => return false,
            };
            matches!(p.mark, Mark::Fill(_))
                && p.composite.simple()
                && p.transfer.is_none()
                && matches!(p.color, crate::color::Color::Solid { .. })
        })
}
// A changed backdrop must never feed an amplifying blend, transfer or mask.
pub(super) fn normal_scene(commands: &[Command]) -> bool {
    commands.iter().all(|command| match command {
        Command::Glyphs(run) => normal_paint(&run.paint),
        Command::Path(p) => {
            matches!(p.color, crate::color::Color::Solid { .. })
                && p.composite.blend == crate::blend::BlendMode::Normal
                && p.composite.mask.is_none()
                && p.transfer.is_none()
        }
        Command::Image(i) => {
            i.composite.blend == crate::blend::BlendMode::Normal
                && i.composite.mask.is_none()
                && i.transfer.is_none()
        }
        Command::Shading(s) => {
            s.composite.blend == crate::blend::BlendMode::Normal
                && s.composite.mask.is_none()
                && s.transfer.is_none()
        }
        Command::Group(g) => {
            g.composite.simple()
                && g.attributes.model.is_none_or(|m| m == Model::Rgb)
                && (!g.attributes.knockout || opaque_knockout(g))
                && normal_scene(&g.commands)
        }
    })
}

fn normal_paint(p: &PathCommand) -> bool {
    matches!(p.color, crate::color::Color::Solid { .. })
        && p.composite.blend == crate::blend::BlendMode::Normal
        && p.composite.mask.is_none()
        && p.transfer.is_none()
}
