//! Static storage admission for Normal source-over (ISO 32000-2, 11.3.3).
//! Counts bound stores at any pixel, independently of the requested tile.
use super::{Command, Model, opaque_knockout};
use crate::color::Color;
use pagura_core::{Error, Limits, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    U8,
    N16,
    F32,
    F64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reason {
    RetainedGroup,
    Pattern,
    Blend,
    Mask,
    Transfer,
    Model,
    Device,
    #[cfg(test)]
    Roundoff,
}

#[derive(Debug)]
pub(super) struct Plan {
    pub(super) format: Format,
    pub(super) work: usize,
    pub(super) side: u32,
    #[cfg(test)]
    pub(super) reason: Option<Reason>,
}

pub(super) fn page(
    list: &crate::display::DisplayList,
    separations: &super::Separations,
) -> Result<Plan> {
    page_policy(list, separations, false)
}

pub(super) fn byte_page(
    list: &crate::display::DisplayList,
    separations: &super::Separations,
) -> Result<Plan> {
    page_policy(list, separations, true)
}

fn page_policy(
    list: &crate::display::DisplayList,
    separations: &super::Separations,
    byte_patterns: bool,
) -> Result<Plan> {
    let root_reason = if separations.enabled {
        Some(Reason::Device)
    } else if list.group.knockout {
        Some(Reason::RetainedGroup)
    } else if list.group.model.is_some_and(|m| m != Model::Rgb) {
        Some(Reason::Model)
    } else {
        None
    };
    if root_reason.is_some() {
        return Ok(Plan {
            format: Format::F64,
            work: 0,
            side: 256,
            #[cfg(test)]
            reason: root_reason,
        });
    }
    let mut count = Count {
        byte_patterns,
        stores: 0,
        dynamic: 0,
        theta: 1.0,
        exact: true,
        reason: None,
        work: 0,
    };
    count.commands(&list.commands, list.limits, 0)?;
    let reason = count.reason;
    // rho = 2^-17; odd source rounding costs at most 2*rho,
    // and pi conservatively includes six relative f32 roundings.
    // Lemma: E <= pi + rho * min(N, 1/theta + K). Reserve 2^-12 levels.
    let gamma = (count.stores as f64).min(1.0 / count.theta + count.dynamic as f64);
    let error = 2.0 / 131_072.0 + 255.0 * 6.0 / 16_777_216.0 + gamma / 131_072.0;
    let format = if reason.is_some() {
        Format::F64
    } else if count.exact {
        Format::N16
    } else if error <= 0.5 - 1.0 / 4096.0 {
        Format::F32
    } else {
        Format::F64
    };
    Ok(Plan {
        format,
        work: if matches!(list.commands.as_slice(), [Command::Image(_)]) {
            0
        } else {
            count.work
        },
        // A single image has no display-list replay cost. Keep its source
        // samples and destination together in cache with a smaller buffer.
        side: if format == Format::F32 && matches!(list.commands.as_slice(), [Command::Image(_)]) {
            128
        } else {
            256
        },
        #[cfg(test)]
        reason: reason.or((format == Format::F64).then_some(Reason::Roundoff)),
    })
}

struct Count {
    byte_patterns: bool,
    stores: usize,
    dynamic: usize,
    theta: f64,
    exact: bool,
    reason: Option<Reason>,
    work: usize,
}
impl Count {
    fn admits_color(&self, color: &Color) -> bool {
        matches!(color, Color::Solid { .. })
            || (self.byte_patterns
                && matches!(color,
            Color::Pattern { selected: Some(pattern), .. } if matches!(pattern.kind, crate::pattern::Kind::Shading(_))))
    }
    fn commands(&mut self, commands: &[Command], limits: Limits, depth: usize) -> Result<()> {
        if depth >= limits.depth {
            return Err(Error::limit("compositing depth exceeded"));
        }
        self.work = self
            .work
            .checked_add(commands.len())
            .filter(|&n| n <= limits.raster_work)
            .ok_or(Error::limit("precision planning work exceeded"))?;
        for command in commands {
            let (state, exact, minimum, transfer, stores) = match command {
                Command::Group(g) => {
                    // Only associative Normal groups share the page buffer. A
                    // retained group, mask source or pattern uses today's f64.
                    if !g.composite.simple()
                        || g.attributes.model.is_some_and(|m| m != Model::Rgb)
                        || (g.attributes.knockout && !opaque_knockout(g))
                    {
                        self.reason.get_or_insert(Reason::RetainedGroup);
                        return Ok(());
                    }
                    self.commands(&g.commands, limits, depth + 1)?;
                    if self.reason.is_some() {
                        return Ok(());
                    }
                    continue;
                }
                Command::Glyphs(run) => {
                    if !self.admits_color(&run.paint.color) {
                        self.reason.get_or_insert(Reason::Pattern);
                    }
                    (
                        &run.paint.composite,
                        matches!(run.paint.color, Color::Solid { .. }),
                        1.0 / 255.0,
                        run.paint.transfer.is_some(),
                        run.glyphs.len(),
                    )
                }
                Command::Path(p) => {
                    if !self.admits_color(&p.color) {
                        self.reason.get_or_insert(Reason::Pattern);
                    }
                    (
                        &p.composite,
                        matches!(p.color, Color::Solid { .. }),
                        1.0 / 255.0,
                        p.transfer.is_some(),
                        1,
                    )
                }
                Command::Image(i) => (&i.composite, false, 0.0, i.transfer.is_some(), 1),
                Command::Shading(s) => (
                    &s.composite,
                    false,
                    if s.shading.antialias {
                        1.0 / 1020.0
                    } else {
                        1.0 / 255.0
                    },
                    s.transfer.is_some(),
                    1,
                ),
            };
            self.stores = self
                .stores
                .checked_add(stores)
                .ok_or(Error::limit("precision store count overflow"))?;
            if state.blend != crate::blend::BlendMode::Normal {
                self.reason.get_or_insert(Reason::Blend);
            }
            if state.mask.is_some() {
                self.reason.get_or_insert(Reason::Mask);
            }
            self.exact &= exact && state.simple();
            if transfer {
                self.reason.get_or_insert(Reason::Transfer);
            }
            // A conservative exclusion fixes this entire page on F64. The
            // existing renderer validates the remaining commands and depths.
            if self.reason.is_some() {
                return Ok(());
            }
            if minimum == 0.0 || state.mask.is_some() {
                self.dynamic = self
                    .dynamic
                    .checked_add(stores)
                    .ok_or(Error::limit("precision dynamic count overflow"))?;
            } else if state.alpha > 0.0 {
                self.theta = self
                    .theta
                    .min(state.alpha * minimum * (1.0 - 1.0 / 4_194_304.0));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        display::PathCommand,
        transparency::{Attributes, Composite, Group},
    };
    use pagura_core::{Position, Rect};
    use pagura_raster::{FillRule, PathBuilder};
    use std::sync::Arc;
    fn page(list: &crate::display::DisplayList) -> Result<Plan> {
        super::page(list, &super::super::Separations::default())
    }
    fn path(alpha: f64) -> Command {
        Command::Path(PathCommand {
            path: Arc::new(PathBuilder::new().finish()),
            mark: crate::display::Mark::Fill(FillRule::NonZero),
            clip: None,
            color: Color::default(),
            position: Position::Unknown,
            transfer: None,
            composite: Composite {
                alpha,
                ..Composite::opaque()
            },
        })
    }
    fn list(commands: Vec<Command>) -> crate::display::DisplayList {
        crate::display::DisplayList {
            commands,
            crop: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 1.0,
            },
            rotation: pagura_document::Rotation::None,
            user_unit: 1.0,
            limits: Limits::default(),
            group: Attributes::default(),
            print_intent: false,
            separations: std::sync::OnceLock::new(),
        }
    }
    #[test]
    fn opaque_and_translucent_sources_have_distinct_storage() {
        let p = page(&list(vec![path(1.0)])).unwrap();
        assert_eq!(p.format, Format::N16);
        assert_eq!(p.work, 1);
        assert_eq!(p.reason, None);
        assert_eq!(page(&list(vec![path(0.5)])).unwrap().format, Format::F32);
        let mut l = list(vec![path(1.0)]);
        l.group.model = Some(Model::Cmyk);
        assert_eq!(page(&l).unwrap().reason, Some(Reason::Model));
        l.group.model = None;
        l.limits.raster_work = 0;
        assert!(page(&l).is_err());
    }
    #[test]
    fn opaque_text_elides_but_translucent_knockout_stays_wide() {
        for (alpha, expected) in [(1.0, Format::N16), (0.5, Format::F64)] {
            let group = Command::Group(Group {
                commands: vec![path(alpha)],
                bounds: None,
                position: Position::Unknown,
                attributes: Attributes {
                    knockout: true,
                    ..Attributes::default()
                },
                composite: Composite::opaque(),
            });
            assert_eq!(page(&list(vec![group])).unwrap().format, expected);
        }
        let Command::Path(mut p) = path(1.0) else {
            unreachable!()
        };
        p.composite.blend = crate::blend::BlendMode::Dodge;
        assert_eq!(
            page(&list(vec![Command::Path(p)])).unwrap().reason,
            Some(Reason::Blend)
        );
    }
    #[test]
    fn tiny_alpha_uniform_stack_cannot_enter_f32() {
        // The independent closed form gives 253.4657168936922; F32
        // stagnates at 255. Admission must reject the 270,000-store stack.
        let l = list((0..270_000).map(|_| path(3.0 / 134_217_728.0)).collect());
        assert_eq!(page(&l).unwrap().reason, Some(Reason::Roundoff));
        let l = list((0..1000).map(|_| path(3.0 / 134_217_728.0)).collect());
        assert_eq!(page(&l).unwrap().format, Format::F32);
    }
    #[test]
    fn glyph_store_bound_counts_instances_instead_of_runs() {
        let Command::Path(paint) = path(3.0 / 134_217_728.0) else {
            unreachable!()
        };
        let mut run = crate::display::GlyphRun::new(paint);
        let mut outline = PathBuilder::new();
        outline.rect(0.0, 0.0, 0.5, 1.0);
        run.push(
            Arc::new(outline.finish()),
            pagura_raster::Transform::IDENTITY,
            Position::Unknown,
            &mut crate::display::GlyphHulls::default(),
        )
        .unwrap();
        run.glyphs.resize(1000, run.glyphs[0]);
        let mut list = list(vec![Command::Glyphs(Box::new(run))]);
        assert_eq!(page(&list).unwrap().format, Format::F32);
        let Command::Glyphs(run) = &mut list.commands[0] else {
            unreachable!()
        };
        run.glyphs.resize(270_000, run.glyphs[0]);
        assert_eq!(page(&list).unwrap().reason, Some(Reason::Roundoff));
    }

    #[test]
    fn glyph_instances_keep_separate_ordered_stores() {
        for (alpha, format, expected) in [(1.0, Format::N16, 63), (0.5, Format::F32, 143)] {
            let Command::Path(paint) = path(alpha) else {
                unreachable!()
            };
            let mut run = crate::display::GlyphRun::new(paint);
            let mut outline = PathBuilder::new();
            outline.rect(0.0, 0.0, 0.5, 1.0);
            let outline = Arc::new(outline.finish());
            for _ in 0..2 {
                run.push(
                    Arc::clone(&outline),
                    pagura_raster::Transform::IDENTITY,
                    Position::Unknown,
                    &mut crate::display::GlyphHulls::default(),
                )
                .unwrap();
            }
            let list = list(vec![Command::Glyphs(Box::new(run))]);
            assert_eq!(page(&list).unwrap().format, format);
            // Each rectangle has coverage byte 128. Clause 11.3.3 gives
            // 255*(127/255)^2 or 255*(191/255)^2, nearest 63 or 143.
            let image = super::super::render(
                &list,
                &crate::RenderOptions::default(),
                crate::Tile {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            )
            .unwrap();
            assert_eq!(image.data(), &[expected, expected, expected, 255]);
        }
    }

    #[test]
    fn simulation_devices_and_transfer_feeds_stay_f64() {
        use crate::OverprintPreview;
        for name in [
            "indexed-base-cmyk",
            "spot-passes-rgb-group",
            "spot-batches-6",
        ] {
            let document = pagura_document::Document::open(
                pagura_testkit::fixture("overprint", &format!("{name}.pdf")),
                &pagura_document::OpenOptions::default(),
            )
            .unwrap();
            let list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
            let options = crate::RenderOptions {
                overprint: OverprintPreview::Always,
                ..Default::default()
            };
            let separations = list.preview_separations(&options).unwrap();
            assert!(separations.enabled, "{name}");
            assert_eq!(
                super::page(&list, &separations).unwrap().reason,
                Some(Reason::Device)
            );
        }
        for name in [
            "transfer-opaque",
            "transfer-image",
            "transfer-shading",
            "transfer-soft-mask-function",
        ] {
            let document = pagura_document::Document::open(
                pagura_testkit::fixture("recovery", &format!("{name}.pdf")),
                &pagura_document::OpenOptions::default(),
            )
            .unwrap();
            let list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
            assert_eq!(page(&list).unwrap().format, Format::F64, "{name}");
        }
    }

    #[test]
    fn generated_general_path_stacks_select_the_proved_precision() {
        for (name, format, expected) in [
            ("two-partial", Format::N16, 219_u8),
            ("byte-threshold", Format::N16, 220),
            ("slivers-254", Format::N16, 94),
            ("slivers-255", Format::N16, 94),
            ("slivers-1000", Format::N16, 5),
            ("slivers-270000", Format::N16, 0),
            ("translucent-stagnation", Format::F32, 11),
            ("tiny-alpha", Format::F64, 253),
            ("steady-state", Format::N16, 128),
        ] {
            let document = pagura_document::Document::open(
                pagura_testkit::fixture("content", &format!("precision-{name}.pdf")),
                &pagura_document::OpenOptions::default(),
            )
            .unwrap();
            let list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
            assert_eq!(page(&list).unwrap().format, format, "{name}");
            let image = list.render(&crate::RenderOptions::default()).unwrap();
            assert_eq!(image.data().len(), 4);
            for &value in &image.data()[..3] {
                assert!(
                    value.abs_diff(expected) <= 1,
                    "{name}: {value}, oracle {expected}"
                );
            }
            assert_eq!(image.data()[3], 255);
        }
    }
}

#[cfg(test)]
mod byte_pattern_tests {
    use super::*;
    use crate::RenderOptions;
    use crate::compositor::Policy;
    use pagura_document::{Document, OpenOptions};
    use pagura_raster::Rgba8;

    #[test]
    fn byte_shading_patterns_match_independent_pixels_and_keep_blend_exclusions() {
        for name in [
            "pattern-shading",
            "pattern-anchored",
            "pattern-stroke",
            "pattern-background",
        ] {
            let document = Document::open(
                pagura_testkit::fixture("content", &format!("{name}.pdf")),
                &OpenOptions::default(),
            )
            .unwrap();
            let mut list = crate::compile_page(&document, document.page(0).unwrap()).unwrap();
            let separations = super::super::Separations::default();
            assert_eq!(page(&list, &separations).unwrap().format, Format::F64);
            assert_eq!(byte_page(&list, &separations).unwrap().format, Format::F32);
            for rows in [3, 16_384] {
                let options = RenderOptions::default();
                let (width, height) = list.device_size(&options).unwrap();
                let output = super::super::render_policy(
                    &list,
                    &options,
                    crate::Tile {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                    Some((
                        Policy {
                            rows,
                            curve_tolerance: None,
                            byte_composition: true,
                            clip_product: false,
                            geometric_compounds: false,
                            fragments: false,
                        },
                        &mut None,
                        Vec::new(),
                    )),
                )
                .unwrap();
                let actual = pagura_testkit::ppm(
                    output.width(),
                    output.height(),
                    &output.to_rgb_over(Rgba8::WHITE),
                );
                let expected = pagura_testkit::fixture("content", &format!("{name}.ppm"));
                assert_eq!(actual.len(), expected.len());
                assert!(
                    actual
                        .iter()
                        .zip(expected.iter())
                        .all(|(a, b)| a.abs_diff(*b) <= 1),
                    "{name}"
                );
            }
            if let Some(Command::Path(path)) = list.commands.first_mut() {
                path.composite.blend = crate::blend::BlendMode::Multiply;
                assert_eq!(byte_page(&list, &separations).unwrap().format, Format::F64);
            }
        }
    }
}
