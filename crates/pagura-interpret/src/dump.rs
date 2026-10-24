//! A deterministic text dump of a [`Scene`], for snapshot tests and
//! debugging: the group tree from the root, then masks, tiling patterns,
//! paints, states, pens and clips, each by its index. Numbers print in
//! their shortest round-trip form, so equal dumps mean equal values.

use crate::model::{Composite, ImageMask, Paint, Shading, ShadingFunction, ShadingKind};
use crate::scene::{Group, GroupId, GroupRole, MarkInfo, Node, NodeId, Scene};
use pagura_color::ColorSpace;
use pagura_raster::{FillRule, Path, Segment, Transform};
use std::fmt::Write as _;

/// The scene as text.
pub fn dump(scene: &Scene) -> String {
    let mut out = String::new();
    let page = &scene.page;
    let _ = writeln!(
        out,
        "page crop {} rotate {} unit {:?} print-intent {}",
        rect(page.crop),
        page.rotation.degrees(),
        page.user_unit,
        scene.print_intent
    );
    group(&mut out, scene, scene.root, 0);
    for (index, mask) in scene.masks.iter().enumerate() {
        let _ = writeln!(
            out,
            "mask #{index} {:?} backdrop {} transfer {} at {:?}",
            mask.kind,
            mask.backdrop
                .as_ref()
                .map_or_else(|| "-".to_owned(), |c| numbers(c)),
            if mask.transfer.is_some() { "yes" } else { "no" },
            mask.position
        );
        group(&mut out, scene, mask.group, 1);
    }
    for (index, tiling) in scene.tilings.iter().enumerate() {
        let _ = writeln!(
            out,
            "tiling #{index} {:?} bbox {} step {} matrix {}",
            tiling.colors,
            rect(tiling.bbox),
            numbers(&tiling.step),
            transform(tiling.matrix)
        );
        group(&mut out, scene, tiling.cell, 1);
    }
    for (index, paint) in scene.paints.iter().enumerate() {
        let _ = writeln!(out, "paint #{index} {}", describe_paint(paint));
    }
    for (index, state) in scene.states.iter().enumerate() {
        let _ = writeln!(out, "state #{index} {}", describe_state(state));
    }
    for (index, pen) in scene.pens.iter().enumerate() {
        let s = &pen.stroke;
        let _ = writeln!(
            out,
            "pen #{index} width {:?} cap {:?} join {:?} miter {:?} dash {} phase {:?} ctm {} {:?}",
            s.width,
            s.cap,
            s.join,
            s.miter_limit,
            numbers(&s.dash_array),
            s.dash_phase,
            transform(pen.transform),
            pen.adjustment
        );
    }
    for (index, clip) in scene.clips.iter().enumerate() {
        let _ = writeln!(
            out,
            "clip #{index} parent {} {} {}",
            clip.parent
                .map_or_else(|| "-".to_owned(), |p| format!("#{}", p.0)),
            rule(clip.rule),
            path(&clip.path)
        );
    }
    out
}

fn group(out: &mut String, scene: &Scene, id: GroupId, depth: usize) {
    let indent = "  ".repeat(depth);
    let Some(g) = scene.group(id) else {
        let _ = writeln!(out, "{indent}group #{} missing", id.0);
        return;
    };
    let _ = writeln!(out, "{indent}group #{} {}", id.0, describe_group(g));
    for &child in &g.children {
        node(out, scene, child, depth + 1);
    }
}

fn node(out: &mut String, scene: &Scene, id: NodeId, depth: usize) {
    let indent = "  ".repeat(depth);
    let Some(node) = scene.node(id) else {
        let _ = writeln!(out, "{indent}node #{} missing", id.0);
        return;
    };
    match node {
        Node::Fill(fill) => {
            let _ = writeln!(
                out,
                "{indent}fill {} paint #{} {} {}",
                rule(fill.rule),
                fill.paint.0,
                mark(&fill.mark),
                path(&fill.path)
            );
        }
        Node::Stroke(stroke) => {
            let _ = writeln!(
                out,
                "{indent}stroke pen #{} paint #{} {} {}",
                stroke.pen.0,
                stroke.paint.0,
                mark(&stroke.mark),
                path(&stroke.path)
            );
        }
        Node::Glyphs(glyphs) => {
            let _ = writeln!(
                out,
                "{indent}glyphs paint #{} {} sources {} placements {}",
                glyphs.paint.0,
                mark(&glyphs.mark),
                glyphs.sources.len(),
                glyphs.placements.len()
            );
            for source in &glyphs.sources {
                let _ = writeln!(
                    out,
                    "{indent}  source font {} glyph {} linear {} segments {}",
                    source.key.font.0,
                    source.key.glyph,
                    transform(source.linear),
                    source.outline.segments().len()
                );
            }
            for (index, placement) in glyphs.placements.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "{indent}  place {} at {:?} {:?} {:?}",
                    placement.source,
                    placement.origin.x,
                    placement.origin.y,
                    glyphs.position(index)
                );
            }
        }
        Node::Image(image) => {
            let _ = writeln!(
                out,
                "{indent}image {}x{} space {} decode {} interpolate {} mask {} stencil {} transform {} {}",
                image.image.source.width(),
                image.image.source.height(),
                image
                    .image
                    .space
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), describe_space),
                image
                    .image
                    .decode
                    .iter()
                    .map(|d| numbers(d))
                    .collect::<Vec<_>>()
                    .join(","),
                image.image.interpolate,
                describe_mask(&image.image.mask),
                image
                    .stencil
                    .map_or_else(|| "-".to_owned(), |p| format!("#{}", p.0)),
                transform(image.transform),
                mark(&image.mark)
            );
        }
        Node::Shading(shading) => {
            let _ = writeln!(
                out,
                "{indent}shading {} transform {} {}",
                describe_shading(&shading.shading),
                transform(shading.transform),
                mark(&shading.mark)
            );
        }
        Node::Group(id) => group(out, scene, *id, depth),
    }
}

fn describe_group(g: &Group) -> String {
    let role = match g.role {
        GroupRole::Page => "page".to_owned(),
        GroupRole::Transparency => "transparency".to_owned(),
        GroupRole::SoftMask { kind } => format!("soft-mask {kind:?}"),
        GroupRole::Tiling => "tiling".to_owned(),
    };
    let a = &g.attributes;
    format!(
        "{role} {:?} isolated {} knockout {} space {:?} bbox {} view {} {}",
        g.cause,
        a.isolated,
        a.knockout,
        a.space,
        g.bbox.as_deref().map_or_else(|| "-".to_owned(), path),
        g.view.map_or_else(
            || "-".to_owned(),
            |v| format!(
                "anchor {:?} {:?} zoom {} rotate {}",
                v.anchor.x, v.anchor.y, !v.no_zoom, !v.no_rotate
            )
        ),
        mark(&g.mark)
    )
}

fn mark(mark: &MarkInfo) -> String {
    format!(
        "state #{} clip {} at {:?}",
        mark.state.0,
        mark.clip
            .map_or_else(|| "-".to_owned(), |c| format!("#{}", c.0)),
        mark.position
    )
}

fn describe_paint(paint: &Paint) -> String {
    match paint {
        Paint::Solid(color) => format!(
            "solid {} {}",
            describe_space(&color.space),
            numbers(&color.components)
        ),
        Paint::Shading(shading) => format!(
            "shading {} matrix {}",
            describe_shading(&shading.shading),
            transform(shading.matrix)
        ),
        Paint::Tiling(tiling) => format!(
            "tiling #{} color {}",
            tiling.pattern.0,
            tiling.color.as_ref().map_or_else(
                || "-".to_owned(),
                |c| format!("{} {}", describe_space(&c.space), numbers(&c.components))
            )
        ),
    }
}

fn describe_state(state: &Composite) -> String {
    format!(
        "alpha {:?} blend {:?} mask {} ais {} overprint {} nonzero {} transfer {} intent {:?} bg {} ucr {}",
        state.alpha,
        state.blend,
        state
            .soft_mask
            .map_or_else(|| "-".to_owned(), |m| format!("#{}", m.0)),
        state.alpha_is_shape,
        state.overprint.enabled,
        state.overprint.nonzero,
        if state.transfer.is_some() {
            "yes"
        } else {
            "no"
        },
        state.conversion.intent,
        if state.conversion.black_generation.is_some() {
            "yes"
        } else {
            "no"
        },
        if state.conversion.undercolor_removal.is_some() {
            "yes"
        } else {
            "no"
        }
    )
}

fn describe_space(space: &ColorSpace) -> String {
    match space {
        ColorSpace::DeviceGray => "DeviceGray".to_owned(),
        ColorSpace::DeviceRgb => "DeviceRGB".to_owned(),
        ColorSpace::DeviceCmyk => "DeviceCMYK".to_owned(),
        ColorSpace::Calibrated(c) => format!("Calibrated({})", c.components()),
        ColorSpace::Icc(icc) => format!(
            "ICCBased({} {} alternate {})",
            icc.components,
            if icc.profile.is_some() {
                "profile"
            } else {
                "no-profile"
            },
            describe_space(&icc.alternate)
        ),
        ColorSpace::Indexed(indexed) => format!(
            "Indexed({} {} bytes {})",
            describe_space(&indexed.base),
            indexed.high,
            indexed.lookup.len()
        ),
        ColorSpace::Separation(s) => format!(
            "Separation({:?} {})",
            s.colorant,
            describe_space(&s.alternate)
        ),
        ColorSpace::DeviceN(d) => format!(
            "DeviceN({:?} {})",
            d.colorants,
            describe_space(&d.alternate)
        ),
        ColorSpace::Pattern(base) => format!(
            "Pattern({})",
            base.as_deref()
                .map_or_else(|| "-".to_owned(), describe_space)
        ),
    }
}

fn describe_mask(mask: &ImageMask) -> String {
    match mask {
        ImageMask::None => "-".to_owned(),
        ImageMask::ColorKey(keys) => format!("color-key {keys:?}"),
        ImageMask::Stencil(image) => {
            format!("stencil {}x{}", image.source.width(), image.source.height())
        }
        ImageMask::Soft { mask, matte } => format!(
            "soft {}x{} matte {}",
            mask.source.width(),
            mask.source.height(),
            matte
                .as_ref()
                .map_or_else(|| "-".to_owned(), |m| numbers(m))
        ),
        ImageMask::Embedded => "embedded".to_owned(),
    }
}

fn describe_shading(shading: &Shading) -> String {
    let kind = match &shading.kind {
        ShadingKind::Function { domain, matrix } => {
            format!(
                "function domain {} matrix {}",
                numbers(domain),
                transform(*matrix)
            )
        }
        ShadingKind::Axial {
            start,
            end,
            domain,
            extend,
        } => format!(
            "axial {:?} {:?} {:?} {:?} domain {} extend {extend:?}",
            start.x,
            start.y,
            end.x,
            end.y,
            numbers(domain)
        ),
        ShadingKind::Radial {
            start,
            start_radius,
            end,
            end_radius,
            domain,
            extend,
        } => format!(
            "radial {:?} {:?} {start_radius:?} {:?} {:?} {end_radius:?} domain {} extend {extend:?}",
            start.x,
            start.y,
            end.x,
            end.y,
            numbers(domain)
        ),
        ShadingKind::Triangles(triangles) => format!("triangles {}", triangles.len()),
        ShadingKind::Patches(patches) => format!("patches {}", patches.len()),
    };
    let function = match &shading.function {
        None => "-".to_owned(),
        Some(ShadingFunction::Single(_)) => "single".to_owned(),
        Some(ShadingFunction::PerComponent(functions)) => {
            format!("per-component {}", functions.len())
        }
    };
    format!(
        "{kind} space {} function {function} background {} bbox {} antialias {}",
        describe_space(&shading.space),
        shading
            .background
            .as_ref()
            .map_or_else(|| "-".to_owned(), |b| numbers(b)),
        shading.bbox.map_or_else(|| "-".to_owned(), rect),
        shading.antialias
    )
}

fn rule(rule: FillRule) -> &'static str {
    match rule {
        FillRule::NonZero => "nonzero",
        FillRule::EvenOdd => "evenodd",
    }
}

fn path(path: &Path) -> String {
    let mut out = String::from("[");
    for (index, segment) in path.segments().iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let _ = match *segment {
            Segment::MoveTo(p) => write!(out, "M {:?} {:?}", p.x, p.y),
            Segment::LineTo(p) => write!(out, "L {:?} {:?}", p.x, p.y),
            Segment::CubicTo { c1, c2, end } => write!(
                out,
                "C {:?} {:?} {:?} {:?} {:?} {:?}",
                c1.x, c1.y, c2.x, c2.y, end.x, end.y
            ),
            Segment::QuadTo { control, end } => write!(
                out,
                "Q {:?} {:?} {:?} {:?}",
                control.x, control.y, end.x, end.y
            ),
            Segment::Close => write!(out, "Z"),
        };
    }
    out.push(']');
    out
}

fn transform(t: Transform) -> String {
    numbers(&[t.a, t.b, t.c, t.d, t.e, t.f])
}

fn rect(r: pagura_core::Rect) -> String {
    numbers(&[r.x0(), r.y0(), r.x1(), r.y1()])
}

fn numbers(values: &[f64]) -> String {
    let parts: Vec<String> = values.iter().map(|v| format!("{v:?}")).collect();
    format!("[{}]", parts.join(" "))
}
