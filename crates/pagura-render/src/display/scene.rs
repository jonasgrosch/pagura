//! One classification of what a command list asks of composition, shared by
//! the opaque renderer and the compositor (11.3.3, 11.4.5, 11.4.6).

use super::{Command, Mark};
use crate::blend::BlendMode;
use crate::color::Color;
use crate::functions::Transfer;
use crate::transparency::{Composite, Group, Model};

/// How a mark paints, whatever its kind.
#[derive(Clone, Copy)]
pub(crate) struct Marking<'a> {
    /// A path's or glyph run's colour; images and shadings paint their own samples.
    pub(crate) color: Option<&'a Color>,
    pub(crate) composite: &'a Composite,
    pub(crate) transfer: Option<&'a Transfer>,
}

impl Marking<'_> {
    fn solid(self) -> bool {
        matches!(self.color, Some(Color::Solid { .. }))
    }
    /// Opaque Normal with a solid colour: the mark replaces what it covers.
    fn opaque(self) -> bool {
        self.composite.simple() && self.solid()
    }
    /// Normal compositing whose result no soft mask or transfer reshapes.
    fn normal(self) -> bool {
        self.color.is_none_or(|_| self.solid())
            && self.composite.blend == BlendMode::Normal
            && self.composite.mask.is_none()
            && self.transfer.is_none()
    }
}

impl Command {
    /// How the mark paints, or `None` for a group.
    pub(crate) fn marking(&self) -> Option<Marking<'_>> {
        let (color, composite, transfer) = match self {
            Self::Path(p) => (Some(&p.color), &p.composite, &p.transfer),
            Self::Glyphs(run) => (
                Some(&run.paint.color),
                &run.paint.composite,
                &run.paint.transfer,
            ),
            Self::Image(i) => (None, &i.composite, &i.transfer),
            Self::Shading(s) => (None, &s.composite, &s.transfer),
            Self::Group(_) => return None,
        };
        Some(Marking {
            color,
            composite,
            transfer: transfer.as_ref(),
        })
    }

    /// Whether the command can change a pixel: a colour space such as
    /// Separation `/None` paints nothing (8.6.6.4).
    pub(crate) fn paints(&self) -> bool {
        match self {
            Self::Path(p) => p.color.paints(),
            Self::Glyphs(run) => run.paint.color.paints(),
            Self::Image(i) => i
                .image
                .space
                .as_ref()
                .map_or_else(|| i.stencil.paints(), crate::color::Space::paints),
            Self::Shading(s) => s.shading.space.paints(),
            Self::Group(_) => true,
        }
    }
}

/// What a command list asks of composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scene {
    /// Opaque solid marks in non-knockout groups that composite without
    /// conversion. Source-over is then associative, so the opaque renderer may
    /// store each mark straight into the 8-bit tile (11.3.3, 11.4.5).
    pub(crate) opaque: bool,
    /// Normal compositing without soft masks or transfers, solid path colours,
    /// and knockout groups of opaque fills only. Such a knockout group reduces
    /// to source-over in its parent (11.4.6). That changes later backdrops only
    /// by rounding, and no amplifying operation magnifies it (ADR 0004).
    pub(crate) normal: bool,
}

impl Scene {
    /// Classifies a command list, every group in it, and the page group.
    pub(crate) fn of(commands: &[Command], page: &crate::transparency::Attributes) -> Self {
        let page = !page.knockout && page.model.is_none_or(|m| m == Model::Rgb);
        let mut scene = Self {
            opaque: page,
            normal: page,
        };
        let mut stack = vec![commands.iter()];
        while let Some(iter) = stack.last_mut() {
            if scene == Self::GENERAL {
                break;
            }
            let Some(command) = iter.next() else {
                stack.pop();
                continue;
            };
            if let Some(marking) = command.marking() {
                scene.opaque &= marking.opaque();
                scene.normal &= marking.normal();
            } else if let Command::Group(group) = command {
                scene.opaque &= opaque_group(group);
                scene.normal &= normal_group(group);
                stack.push(group.commands.iter());
            }
        }
        scene
    }

    const GENERAL: Self = Self {
        opaque: false,
        normal: false,
    };
}

/// A non-isolated group composites in its parent's blending space (11.7.2);
/// an isolated one converts unless its space is the page's RGB.
fn unconverted(group: &Group) -> bool {
    !group.attributes.isolated || group.attributes.model.is_none_or(|m| m == Model::Rgb)
}

fn opaque_group(group: &Group) -> bool {
    group.composite.simple()
        && group.attributes.view.is_none()
        && !group.attributes.knockout
        && unconverted(group)
}

fn normal_group(group: &Group) -> bool {
    group.composite.simple()
        && unconverted(group)
        && (!group.attributes.knockout || opaque_knockout(group))
}

/// A knockout group whose members are all opaque fills: each replaces what it
/// covers by shape, which is source-over (11.4.6).
pub(crate) fn opaque_knockout(group: &Group) -> bool {
    group.attributes.view.is_none()
        && !group.attributes.isolated
        && group.commands.iter().all(|command| {
            let p = match command {
                Command::Path(p) => p,
                Command::Glyphs(run) => &run.paint,
                _ => return false,
            };
            matches!(p.mark, Mark::Fill(_))
                && command
                    .marking()
                    .is_some_and(|m| m.opaque() && m.transfer.is_none())
        })
}
