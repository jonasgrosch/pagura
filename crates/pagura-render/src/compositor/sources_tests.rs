//! Required source validation remains visible across culling and execution.
use super::*;
use crate::color::{Color, Space};
use crate::display::{GlyphHulls, GlyphRun};
use crate::functions::Function;
use crate::transparency::Attributes;
use pagura_core::{Dictionary, ErrorKind, Name, ObjectId, Position, Rect, Value};
use pagura_document::{Document, OpenOptions, Rotation};
use pagura_raster::Stroke;

fn position(offset: usize) -> Position {
    Position::Stream {
        object: ObjectId::new(17, 0),
        offset,
    }
}

fn gray() -> Color {
    Color::Solid {
        space: Space::Gray,
        components: vec![0.25],
    }
}

fn path(x: f64, width: f64, color: Color, offset: usize) -> PathCommand {
    let mut path = PathBuilder::new();
    path.rect(x, 0.0, width, 1.0);
    PathCommand {
        path: Arc::new(path.finish()),
        mark: Mark::Fill(FillRule::NonZero),
        clip: None,
        color,
        position: position(offset),
        transfer: None,
        composite: Composite::opaque(),
    }
}

fn singular(mut paint: PathCommand) -> PathCommand {
    paint.mark = Mark::Stroke(Stroke::default(), Transform::scale(0.0, 1.0), false);
    paint
}

fn list(commands: Vec<Command>) -> DisplayList {
    DisplayList {
        commands,
        crop: Rect::from_corners(0.0, 0.0, 1.0, 1.0).unwrap(),
        rotation: Rotation::None,
        user_unit: 1.0,
        limits: Limits::default(),
        group: Attributes::default(),
        print_intent: false,
        separations: std::sync::OnceLock::new(),
    }
}

fn context(list: &DisplayList) -> Context {
    assert!(list.plain_opaque());
    let separations = Arc::new(Separations::default());
    let plan = plan::page(list, &separations).unwrap();
    Context::new(list, &RenderOptions::default(), &plan, separations).unwrap()
}

fn surface() -> Surface {
    Surface::with_buffer(
        Tile {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        narrow::Buffer::new(plan::Format::N16, 1, [255; 4]).unwrap(),
    )
}

fn execute(context: &mut Context, list: &DisplayList, target: &mut Surface) -> Result<()> {
    context.commands(
        &list.commands,
        target,
        Backdrop::Transparent,
        Model::Rgb,
        false,
        0,
    )
}

fn pixel(target: &Surface) -> [u8; 4] {
    let mut pixel = [0; 4];
    target.narrow().unwrap().output(0, &mut pixel);
    pixel
}

fn function(low: f64, high: f64, operations: usize) -> Function {
    let document = Document::open(
        pagura_testkit::fixture("content", "fractional.pdf"),
        &OpenOptions {
            limits: Limits {
                operations,
                ..Limits::default()
            },
            ..OpenOptions::default()
        },
    )
    .unwrap();
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"FunctionType"), Value::Integer(2));
    dictionary.insert(
        Name::new(b"Domain"),
        Value::Array(vec![Value::Integer(0), Value::Integer(1)]),
    );
    dictionary.insert(Name::new(b"C0"), Value::Array(vec![Value::Real(low)]));
    dictionary.insert(Name::new(b"C1"), Value::Array(vec![Value::Real(high)]));
    dictionary.insert(Name::new(b"N"), Value::Integer(1));
    Function::parse(&document, &Value::Dictionary(dictionary), 1, 1).unwrap()
}

fn tint(function: Function) -> Color {
    Color::Solid {
        space: Space::Tint {
            names: vec![Name::new(b"Ink")].into(),
            base: Arc::new(Space::Gray),
            function: Arc::new(function),
        },
        components: vec![0.25],
    }
}

#[test]
fn validation_and_paint_share_the_function_budget() {
    let list = list(vec![Command::Path(path(
        0.0,
        1.0,
        tint(function(0.0, 1.0, 2)),
        10,
    ))]);
    let mut context = context(&list);
    let mut target = surface();
    context.validate_plain_sources(&list.commands, 0).unwrap();
    assert_eq!(context.function_work, 1);
    execute(&mut context, &list, &mut target).unwrap();
    assert_eq!(context.function_work, 2);
    assert_eq!(pixel(&target), [64, 64, 64, 255]);
    let error = execute(&mut context, &list, &mut target).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.position, position(10));
    assert_eq!(context.function_work, 2);
    assert_eq!(pixel(&target), [64, 64, 64, 255]);
}

#[test]
fn validation_command_work_is_not_reset_before_execution() {
    let list = list(vec![Command::Path(path(0.0, 1.0, gray(), 10))]);
    let mut context = context(&list);
    let budget = context.work + 1;
    context.limits.raster_work = budget;
    context.validate_plain_sources(&list.commands, 0).unwrap();
    assert_eq!(context.work, budget);
    let mut target = surface();
    assert_eq!(
        execute(&mut context, &list, &mut target).unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!(context.work, budget);
    assert_eq!(pixel(&target), [255; 4]);
}

#[test]
fn off_page_stroke_errors_do_not_apply_to_glyph_paint_metadata() {
    let paths = list(vec![Command::Path(singular(path(10.0, 1.0, gray(), 20)))]);
    let paint = singular(path(10.0, 1.0, gray(), 20));
    let outline = Arc::clone(&paint.path);
    let mut run = GlyphRun::new(paint);
    run.push(
        outline,
        Transform::IDENTITY,
        position(21),
        &mut GlyphHulls::default(),
    )
    .unwrap();
    let glyphs = list(vec![Command::Glyphs(Box::new(run))]);
    for accuracy in [crate::Accuracy::Exact, crate::Accuracy::Competitive] {
        let options = RenderOptions {
            accuracy,
            ..RenderOptions::default()
        };
        let error = paths.render(&options).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(crate::features::SINGULAR_STROKE)
        );
        assert_eq!(error.position, position(20));
        assert_eq!(glyphs.render(&options).unwrap().data(), [255; 4]);
    }
}

#[test]
fn none_skips_source_painting_but_still_validates_path_strokes() {
    let none = Color::Solid {
        space: Space::Special {
            inputs: 1,
            all: false,
        },
        components: vec![0.25],
    };
    let blank = list(vec![Command::Path(path(0.0, 1.0, none.clone(), 30))]);
    let invalid = list(vec![Command::Path(singular(path(10.0, 1.0, none, 31)))]);
    for accuracy in [crate::Accuracy::Exact, crate::Accuracy::Competitive] {
        let options = RenderOptions {
            accuracy,
            ..RenderOptions::default()
        };
        assert_eq!(blank.render(&options).unwrap().data(), [255; 4]);
        let error = invalid.render(&options).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(crate::features::SINGULAR_STROKE)
        );
        assert_eq!(error.position, position(31));
    }
}

#[test]
fn nested_validation_has_a_depth_bound_and_keeps_inner_context() {
    let group = |offset, commands| {
        Command::Group(Group {
            position: position(offset),
            bounds: None,
            commands,
            attributes: Attributes::default(),
            composite: Composite::opaque(),
        })
    };
    let list = list(vec![group(
        40,
        vec![group(41, vec![Command::Path(path(10.0, 1.0, gray(), 42))])],
    )]);
    let mut too_shallow = context(&list);
    too_shallow.limits.depth = 2;
    let error = too_shallow
        .validate_plain_sources(&list.commands, 0)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
    assert_eq!(error.position, position(41));
    let mut admitted = context(&list);
    admitted.limits.depth = 3;
    admitted.validate_plain_sources(&list.commands, 0).unwrap();
}

#[test]
fn transfers_stay_deferred_wide_and_reject_fractional_selection() {
    // Finite parsed parameters can still overflow during evaluation.
    let broken = function(1e308, -1e308, Limits::default().operations);
    assert_eq!(
        broken.evaluate(&[0.25]).unwrap_err().kind,
        ErrorKind::Syntax
    );
    let inverse = function(1.0, 0.0, Limits::default().operations);
    for (x, width, function, expected) in [
        (10.0, 1.0, broken, Some([255; 4])),
        (0.0, 1.0, inverse.clone(), Some([191, 191, 191, 255])),
        (0.0, 0.5, inverse, None),
    ] {
        let mut paint = path(x, width, gray(), 50);
        paint.transfer = Some(Arc::new([function.clone(), function.clone(), function]));
        let list = list(vec![Command::Path(paint)]);
        assert!(list.plain_opaque());
        assert_eq!(
            plan::page(&list, &Separations::default()).unwrap().format,
            plan::Format::F64
        );
        for accuracy in [crate::Accuracy::Exact, crate::Accuracy::Competitive] {
            let options = RenderOptions {
                accuracy,
                ..RenderOptions::default()
            };
            let result = list.render(&options);
            if let Some(expected) = expected {
                assert_eq!(result.unwrap().data(), expected);
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    error.kind,
                    ErrorKind::Unsupported(crate::features::TRANSPARENCY_TRANSFER)
                );
                assert_eq!(error.position, position(50));
            }
        }
    }
}

#[test]
fn off_page_stroke_inverse_overflow_remains_an_error() {
    let mut paint = path(1e200, 1e200, gray(), 60);
    paint.mark = Mark::Stroke(Stroke::default(), Transform::scale(1e-200, 1.0), false);
    let list = list(vec![Command::Path(paint)]);
    for accuracy in [crate::Accuracy::Exact, crate::Accuracy::Competitive] {
        let error = list
            .render(&RenderOptions {
                accuracy,
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidArgument);
        assert_eq!(error.message, "path geometry is not finite");
        assert_eq!(error.position, position(60));
    }
}
