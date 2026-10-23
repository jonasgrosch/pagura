//! Experimental presentation adapter for the public recorded scene.
//!
//! Geometry stays vector geometry: no CPU coverage spans enter this exporter.
//! The browser owns antialiasing, sampling and compositing. Unsupported PDF
//! semantics refuse the whole export; no incomplete page is called success.

mod clips;
mod images;

use pagura_color::{ColorSpace, Converter, Process, Target};
use pagura_core::{Error, Limits, Meter, Rect};
use pagura_document::Document;
use pagura_interpret::{
    BlendMode, Composite, FillNode, GroupId, GroupRole, MarkInfo, Node, OutlineKey, Paint, PaintId,
    Scene, StateId,
};
use pagura_raster::{FillRegion, FillRule, LineCap, LineJoin, Path, Segment, Transform};
use std::collections::HashMap;
use std::fmt::{self, Write};

#[derive(Debug)]
pub enum Failure {
    Unsupported(&'static str),
    Engine(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Engine(error)
    }
}

type Result<T> = std::result::Result<T, Failure>;

pub struct Export {
    pub bytes: Vec<u8>,
    pub paths: usize,
    pub glyphs: usize,
    pub images: usize,
    pub clips_elided: usize,
    pub clips_retained: usize,
}

/// All attributes come from finite numbers or fixed strings; PDF strings,
/// links, scripts, external resources and font names never enter the XML.
pub fn export(scene: &Scene, document: &Document) -> Result<Export> {
    if scene.print_intent {
        return Err(Failure::Unsupported(
            "SVG does not simulate PDF print overprint",
        ));
    }
    let limits = document.limits();
    let mut writer = Writer {
        scene,
        document,
        limits,
        defs: Xml::new(),
        body: Xml::new(),
        work: Meter::new(limits.render_work, "SVG export work budget exceeded"),
        functions: Meter::new(
            limits.function_work,
            "SVG colour function work budget exceeded",
        ),
        image_bytes: Meter::new(limits.decoded_bytes, "SVG image byte budget exceeded"),
        colors: HashMap::new(),
        glyph_ids: HashMap::new(),
        image_ids: HashMap::new(),
        paths: 0,
        glyphs: 0,
        clip_bounds: Vec::new(),
        clips_elided: 0,
        clips_retained: 0,
    };
    writer.clip_bounds = clips::chains(scene, &mut writer.work)?;
    for (i, clip) in scene.clips.iter().enumerate() {
        writer.defs.add(format_args!(
            "<clipPath id=\"c{i}\" clipPathUnits=\"userSpaceOnUse\">"
        ))?;
        writer
            .defs
            .add(format_args!("<path clip-rule=\"{}\"", rule(clip.rule)))?;
        if let Some(parent) = clip.parent {
            writer
                .defs
                .add(format_args!(" clip-path=\"url(#c{})\"", parent.0))?;
        }
        writer.defs.add(format_args!(" d=\""))?;
        path(&mut writer.defs, &clip.path, &mut writer.work)?;
        writer.defs.add(format_args!("\"/>"))?;
        writer.defs.add(format_args!("</clipPath>"))?;
    }
    writer.group(scene.root, 0)?;
    let mut xml = Xml::new();
    let (mut extent_width, mut extent_height) = (
        scene.page.crop.width() * scene.page.user_unit,
        scene.page.crop.height() * scene.page.user_unit,
    );
    if matches!(
        scene.page.rotation,
        pagura_document::Rotation::Clockwise90 | pagura_document::Rotation::Clockwise270
    ) {
        std::mem::swap(&mut extent_width, &mut extent_height);
    }
    xml.add(format_args!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{extent_width}\" height=\"{extent_height}\" viewBox=\"0 0 {extent_width} {extent_height}\" color-interpolation=\"sRGB\" style=\"isolation:isolate;background:white\"><defs>{}</defs><rect width=\"100%\" height=\"100%\" fill=\"white\"/><g transform=\"{}\">{}</g></svg>",
        writer.defs.text, matrix(scene.page.device_transform(1.0))?, writer.body.text,
    ))?;
    Ok(Export {
        bytes: xml.text.into_bytes(),
        paths: writer.paths,
        glyphs: writer.glyphs,
        images: writer.image_ids.len(),
        clips_elided: writer.clips_elided,
        clips_retained: writer.clips_retained,
    })
}

struct Writer<'a> {
    scene: &'a Scene,
    document: &'a Document,
    limits: Limits,
    defs: Xml,
    body: Xml,
    work: Meter,
    functions: Meter,
    image_bytes: Meter,
    colors: HashMap<(PaintId, StateId), String>,
    glyph_ids: HashMap<OutlineKey, (usize, Option<Rect>)>,
    image_ids: HashMap<(pagura_image::ImageId, StateId, Option<PaintId>), usize>,
    paths: usize,
    glyphs: usize,
    clip_bounds: Vec<Option<Rect>>,
    clips_elided: usize,
    clips_retained: usize,
}

impl Writer<'_> {
    fn group(&mut self, id: GroupId, depth: usize) -> Result<()> {
        self.work.charge(1)?;
        if depth > self.limits.depth {
            return Err(Error::limit("SVG group depth exceeded").into());
        }
        let group = &self.scene.groups[id.index()];
        if group.view.is_some() {
            return Err(Failure::Unsupported(
                "SVG fixed-view annotations are not implemented",
            ));
        }
        if group
            .attributes
            .space
            .is_some_and(|space| space != Process::Rgb)
        {
            return Err(Failure::Unsupported("SVG groups require RGB blending"));
        }
        if group.attributes.knockout && !self.opaque(id, depth)? {
            return Err(Failure::Unsupported(
                "SVG cannot represent this PDF knockout group",
            ));
        }
        if group.role != GroupRole::Page && !group.attributes.isolated && !self.normal(id, depth)? {
            return Err(Failure::Unsupported(
                "SVG non-isolated PDF blend groups are not implemented",
            ));
        }
        let page = group.role == GroupRole::Page;
        if !page {
            self.begin(group.mark)?;
        }
        self.body.add(format_args!(
            "<g{}>",
            // A PDF page composites as an isolated group, including when
            // its dictionary has no explicit Group entry. The white output
            // backdrop is outside that group, never a blend input inside it.
            if page || group.attributes.isolated {
                " style=\"isolation:isolate\""
            } else {
                ""
            }
        ))?;
        for &child in &group.children {
            self.node(
                self.scene
                    .node(child)
                    .ok_or(Error::invalid_argument("invalid SVG scene node"))?,
                depth + 1,
            )?;
        }
        self.body.add(format_args!("</g>"))?;
        if !page {
            self.end(group.mark)?;
        }
        Ok(())
    }

    fn opaque(&mut self, id: GroupId, depth: usize) -> Result<bool> {
        self.inspect(id, depth, true)
    }

    fn normal(&mut self, id: GroupId, depth: usize) -> Result<bool> {
        self.inspect(id, depth, false)
    }

    fn inspect(&mut self, id: GroupId, depth: usize, opaque: bool) -> Result<bool> {
        if depth > self.limits.depth {
            return Err(Error::limit("SVG group depth exceeded").into());
        }
        for &child in &self.scene.groups[id.index()].children {
            self.work.charge(1)?;
            let mark = match &self.scene.nodes[child.index()] {
                Node::Fill(n) => n.mark,
                Node::Stroke(n) => n.mark,
                Node::Glyphs(n) => n.mark,
                Node::Image(_) | Node::Shading(_) if opaque => return Ok(false),
                Node::Image(n) => n.mark,
                Node::Shading(n) => n.mark,
                Node::Group(g) => {
                    if !self.inspect(*g, depth + 1, opaque)? {
                        return Ok(false);
                    }
                    self.scene.groups[g.index()].mark
                }
            };
            let state = &self.scene.states[mark.state.index()];
            if state.blend != BlendMode::Normal || opaque && !state.is_plain() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn node(&mut self, node: &Node, depth: usize) -> Result<()> {
        self.work.charge(1)?;
        match node {
            Node::Group(id) => self.group(*id, depth),
            Node::Fill(node) => self.fill(node),
            Node::Stroke(node) => {
                if point_only(&node.path) {
                    return Err(Failure::Unsupported(
                        "SVG PDF zero-length stroke painting is not implemented",
                    ));
                }
                let color = self.color(node.paint, node.mark.state)?;
                self.begin(node.mark)?;
                let pen = &self.scene.pens[node.pen.index()];
                let inverse = pen
                    .transform
                    .inverse()
                    .ok_or(Failure::Unsupported("SVG singular stroke transform"))?;
                let local = node.path.transformed(inverse);
                let stroke = &pen.stroke;
                self.body.add(format_args!("<path fill=\"none\" stroke=\"{color}\" transform=\"{}\" stroke-width=\"{}\" stroke-linecap=\"{}\" stroke-linejoin=\"{}\" stroke-miterlimit=\"{}\" stroke-dashoffset=\"{}\"", matrix(pen.transform)?, if stroke.width == 0.0 {1.0} else {stroke.width}, match stroke.cap {LineCap::Butt=>"butt",LineCap::Round=>"round",LineCap::ProjectingSquare=>"square"}, match stroke.join {LineJoin::Miter=>"miter",LineJoin::Round=>"round",LineJoin::Bevel=>"bevel"}, stroke.miter_limit, stroke.dash_phase))?;
                if stroke.width == 0.0 {
                    self.body
                        .add(format_args!(" vector-effect=\"non-scaling-stroke\""))?;
                }
                if !stroke.dash_array.is_empty() {
                    self.body.add(format_args!(" stroke-dasharray=\""))?;
                    for dash in &stroke.dash_array {
                        self.work.charge(1)?;
                        self.body.add(format_args!("{dash} "))?;
                    }
                    self.body.add(format_args!("\""))?;
                }
                self.body.add(format_args!(" d=\""))?;
                path(&mut self.body, &local, &mut self.work)?;
                self.body.add(format_args!("\"/>"))?;
                self.paths += 1;
                self.end(node.mark)
            }
            Node::Glyphs(node) => {
                let color = self.color(node.paint, node.mark.state)?;
                // Clip and opacity apply to each glyph, preserving PDF show
                // order and overlap rather than painting a compound run once.
                for place in &node.placements {
                    self.work.charge(1)?;
                    let source = &node.sources[place.source as usize];
                    let (id, bounds) = if let Some(&resource) = self.glyph_ids.get(&source.key) {
                        resource
                    } else {
                        let id = self.glyph_ids.len();
                        self.defs.add(format_args!("<path id=\"g{id}\" d=\""))?;
                        path(&mut self.defs, &source.outline, &mut self.work)?;
                        self.defs.add(format_args!("\"/>"))?;
                        let bounds = clips::hull(&source.outline, &mut self.work)?;
                        self.glyph_ids.insert(source.key, (id, bounds));
                        (id, bounds)
                    };
                    let transform = source
                        .linear
                        .then(Transform::translate(place.origin.x, place.origin.y));
                    let mark = self.fill_mark(node.mark, clips::transformed(bounds, transform));
                    self.begin(mark)?;
                    self.body.add(format_args!(
                        "<use href=\"#g{id}\" fill=\"{color}\" transform=\"{}\"/>",
                        matrix(transform)?
                    ))?;
                    self.end(mark)?;
                    self.glyphs += 1;
                }
                Ok(())
            }
            Node::Image(node) => self.image(node),
            Node::Shading(_) => Err(Failure::Unsupported("SVG PDF shadings are not implemented")),
        }
    }

    fn fill(&mut self, node: &FillNode) -> Result<()> {
        if matches!(
            node.path.fill_region(node.rule),
            Some(FillRegion::Points | FillRegion::EmptyBoundary)
        ) {
            return Err(Failure::Unsupported(
                "SVG PDF zero-area fill painting is not implemented",
            ));
        }
        let color = self.color(node.paint, node.mark.state)?;
        let bounds = clips::hull(&node.path, &mut self.work)?;
        let mark = self.fill_mark(node.mark, clips::transformed(bounds, Transform::IDENTITY));
        self.begin(mark)?;
        self.body.add(format_args!(
            "<path fill=\"{color}\" fill-rule=\"{}\" d=\"",
            rule(node.rule)
        ))?;
        path(&mut self.body, &node.path, &mut self.work)?;
        self.body.add(format_args!("\"/>"))?;
        self.paths += 1;
        self.end(mark)
    }

    fn fill_mark(&mut self, mut mark: MarkInfo, bounds: Option<Rect>) -> MarkInfo {
        if clips::inactive(mark, bounds, &self.clip_bounds, self.scene.page.user_unit) {
            mark.clip = None;
            self.clips_elided += 1;
        }
        mark
    }

    fn color(&mut self, paint: PaintId, state: StateId) -> Result<String> {
        if let Some(color) = self.colors.get(&(paint, state)) {
            return Ok(color.clone());
        }
        let Paint::Solid(color) = self.scene.paints[paint.index()].as_ref() else {
            return Err(Failure::Unsupported(
                "SVG shading and tiling paints are not implemented",
            ));
        };
        if matches!(color.space.as_ref(), ColorSpace::Pattern(_)) {
            return Err(Failure::Unsupported(
                "SVG initial pattern colour is not implemented",
            ));
        }
        let converter = Converter::new(
            &color.space,
            &Target::new(Process::Rgb),
            &self.scene.states[state.index()].conversion,
        )?;
        self.work.charge(converter.cost())?;
        self.functions.charge(converter.cost())?;
        let mut rgb = [0.0; 3];
        let painted = converter.convert(&color.components, &mut rgb);
        let color = if painted.0 == 0 {
            String::from("none")
        } else {
            rgb_color(rgb)
        };
        self.colors.insert((paint, state), color.clone());
        Ok(color)
    }

    fn begin(&mut self, mark: MarkInfo) -> Result<()> {
        let state = &self.scene.states[mark.state.index()];
        validate_state(state)?;
        if let Some(clip) = mark.clip {
            self.clips_retained += 1;
            self.body
                .add(format_args!("<g clip-path=\"url(#c{})\">", clip.0))?;
        }
        if state.alpha != 1.0 || state.blend != BlendMode::Normal {
            self.body.add(format_args!(
                "<g opacity=\"{}\" style=\"mix-blend-mode:{}\">",
                state.alpha,
                blend(state.blend)
            ))?;
        }
        Ok(())
    }

    fn end(&mut self, mark: MarkInfo) -> Result<()> {
        let state = &self.scene.states[mark.state.index()];
        if state.alpha != 1.0 || state.blend != BlendMode::Normal {
            self.body.add(format_args!("</g>"))?;
        }
        if mark.clip.is_some() {
            self.body.add(format_args!("</g>"))?;
        }
        Ok(())
    }
}

fn validate_state(state: &Composite) -> Result<()> {
    if state.soft_mask.is_some() {
        return Err(Failure::Unsupported(
            "SVG PDF soft-mask groups are not implemented",
        ));
    }
    if state.transfer.is_some() {
        return Err(Failure::Unsupported(
            "SVG PDF transfer functions are not implemented",
        ));
    }
    if state.overprint.enabled {
        return Err(Failure::Unsupported("SVG does not simulate PDF overprint"));
    }
    if !state.alpha.is_finite() {
        return Err(Error::invalid_argument("non-finite SVG opacity").into());
    }
    Ok(())
}

fn rgb_color(rgb: [f64; 3]) -> String {
    format!(
        "rgb({}% {}% {}%)",
        rgb[0].clamp(0.0, 1.0) * 100.0,
        rgb[1].clamp(0.0, 1.0) * 100.0,
        rgb[2].clamp(0.0, 1.0) * 100.0
    )
}

fn rule(rule: FillRule) -> &'static str {
    match rule {
        FillRule::NonZero => "nonzero",
        FillRule::EvenOdd => "evenodd",
    }
}

fn blend(blend: BlendMode) -> &'static str {
    match blend {
        BlendMode::Normal => "normal",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Overlay => "overlay",
        BlendMode::Darken => "darken",
        BlendMode::Lighten => "lighten",
        BlendMode::ColorDodge => "color-dodge",
        BlendMode::ColorBurn => "color-burn",
        BlendMode::HardLight => "hard-light",
        BlendMode::SoftLight => "soft-light",
        BlendMode::Difference => "difference",
        BlendMode::Exclusion => "exclusion",
        BlendMode::Hue => "hue",
        BlendMode::Saturation => "saturation",
        BlendMode::Color => "color",
        BlendMode::Luminosity => "luminosity",
    }
}

fn matrix(t: Transform) -> Result<String> {
    if ![t.a, t.b, t.c, t.d, t.e, t.f].iter().all(|n| n.is_finite()) {
        return Err(Error::invalid_argument("non-finite SVG transform").into());
    }
    Ok(format!(
        "matrix({} {} {} {} {} {})",
        t.a, t.b, t.c, t.d, t.e, t.f
    ))
}

fn path(xml: &mut Xml, path: &Path, work: &mut Meter) -> Result<()> {
    for segment in path.segments() {
        work.charge(1)?;
        match *segment {
            Segment::MoveTo(p) | Segment::LineTo(p) => {
                if !p.is_finite() {
                    return Err(Error::invalid_argument("non-finite SVG path").into());
                }
                let command = if matches!(segment, Segment::MoveTo(_)) {
                    'M'
                } else {
                    'L'
                };
                xml.add(format_args!("{command}{} {} ", p.x, p.y))?;
            }
            Segment::CubicTo { c1, c2, end } => {
                if ![c1, c2, end].iter().all(|p| p.is_finite()) {
                    return Err(Error::invalid_argument("non-finite SVG curve").into());
                }
                xml.add(format_args!(
                    "C{} {} {} {} {} {} ",
                    c1.x, c1.y, c2.x, c2.y, end.x, end.y
                ))?;
            }
            Segment::Close => xml.add(format_args!("Z "))?,
        }
    }
    Ok(())
}

fn point_only(path: &Path) -> bool {
    let mut point = None;
    path.segments().iter().all(|segment| match *segment {
        Segment::MoveTo(p) | Segment::LineTo(p) => {
            let same = point.is_none_or(|first| first == p);
            point = Some(p);
            same
        }
        Segment::CubicTo { c1, c2, end } => point.is_some_and(|p| p == c1 && p == c2 && p == end),
        Segment::Close => true,
    })
}

/// Each XML buffer and the final output are bounded, including formatting.
struct Xml {
    text: String,
    tags: usize,
}
impl Xml {
    fn new() -> Self {
        Self {
            text: String::new(),
            tags: 0,
        }
    }
    fn add(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
        self.write_fmt(args)
            .map_err(|_| Failure::Engine(Error::limit("SVG export exceeds byte or element budget")))
    }
}
impl Write for Xml {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.tags = self
            .tags
            .checked_add(value.bytes().filter(|&b| b == b'<').count())
            .filter(|&n| n <= 200_000)
            .ok_or(fmt::Error)?;
        if self
            .text
            .len()
            .checked_add(value.len())
            .is_none_or(|n| n > super::MAX_FRAME)
        {
            return Err(fmt::Error);
        }
        self.text.try_reserve(value.len()).map_err(|_| fmt::Error)?;
        self.text.push_str(value);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests;
