//! Calculator programs: static validation and evaluation against the
//! previous interpreter, on hand-written programs, random typed programs and
//! every Type 4 function in the generated corpus.

use super::{Pair, compare, inputs};
use crate::function::tests::oracle::{Oracle, unlimited};
use crate::function::tests::random::Rng;
use crate::function::{CalculatorFunction, Function};
use pagura_core::{ErrorKind, Limits, Position};
use std::sync::Arc;

/// The parameters of `program` with `inputs` inputs; a function has at
/// least one, so a program written for none first pops a dummy input.
fn parameters(program: &str, inputs: usize, outputs: usize) -> CalculatorFunction {
    let program = if inputs == 0 {
        program.replacen('{', "{ pop", 1)
    } else {
        program.to_owned()
    };
    CalculatorFunction {
        domain: vec![[-2.0, 2.0]; inputs.max(1)],
        range: vec![[-1e9, 1e9]; outputs],
        program: program.as_bytes().into(),
    }
}

fn pair(parameters: &CalculatorFunction) -> Pair {
    let limits = Limits::default();
    Pair {
        old: Oracle::calculator(parameters, &limits).unwrap(),
        new: Arc::new(
            Function::calculator(parameters.clone(), &limits).unwrap_or_else(|e| {
                panic!("{}: {e}", String::from_utf8_lossy(&parameters.program))
            }),
        ),
    }
}

fn evaluate(program: &str, inputs: &[f64], outputs: usize) -> Vec<f64> {
    let function = Function::calculator(parameters(program, inputs.len(), outputs), &Limits::default())
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    let mut out = vec![0.0; outputs];
    function.evaluate(if inputs.is_empty() { &[0.0] } else { inputs }, &mut out);
    out
}

/// Programs over every operator, with the inputs and outputs they take.
const PROGRAMS: &[(&str, usize, usize)] = &[
    ("{ 2 3 add 4 mul }", 0, 1),
    ("{ -6.5 round }", 0, 1),
    ("{ 90 sin }", 0, 1),
    ("{ 1 2 lt {4} {9} ifelse }", 0, 1),
    ("{ 1 2 2 copy add add add }", 0, 1),
    ("{ 1 2 3 3 -1 roll pop pop }", 0, 1),
    ("{ -1 -1 bitshift }", 0, 1),
    ("{ 2147483647 1 add }", 0, 1),
    ("{ false {1 0 div} if 7 }", 0, 1),
    ("{ 10 3 idiv 10 3 mod add }", 0, 1),
    ("{ -2147483648 -1 mod }", 0, 1),
    ("{ 3 cvr 2 exp sqrt }", 0, 1),
    ("{ true false or not {0} {5} ifelse }", 0, 1),
    ("{ add dup 1 exch sub exch 0 }", 2, 3),
    ("{ 1 exch sub }", 1, 1),
    ("{ dup mul exch dup mul add sqrt }", 2, 1),
    ("{ dup 0 gt { 2 mul } { neg } ifelse }", 1, 1),
    ("{ abs ceiling exch floor }", 2, 2),
    ("{ truncate exch round cvi }", 2, 2),
    ("{ 1 index 1 index add 3 1 roll pop pop }", 2, 1),
    ("{ 360 mul cos exch 45 add sin }", 2, 2),
    ("{ 1 add ln exch 3 add log }", 2, 2),
    ("{ atan }", 2, 1),
    ("{ 2 copy atan 3 1 roll exp }", 2, 2),
    ("{ 2 copy eq {1} {0} ifelse 3 1 roll ne {1} {0} ifelse }", 2, 2),
    ("{ 2 copy ge 3 1 roll le and {1} {0} ifelse }", 2, 1),
    ("{ 2 copy gt 3 1 roll lt xor {1} {0} ifelse }", 2, 1),
    ("{ 10 mul cvi 3 and exch 10 mul cvi 5 or xor }", 2, 1),
    ("{ 100 mul cvi 7 mod 3 bitshift -1 bitshift }", 1, 1),
    ("{ 100 mul cvi 3 idiv neg abs }", 1, 1),
    ("{ 1 0.5 lt { 0.25 } { 0.75 } ifelse mul }", 1, 1),
    ("{ dup 0.5 ge { dup 0.75 ge { pop 1 } { pop 0.5 } ifelse } { pop 0 } ifelse }", 1, 1),
    ("{ 0 1 2 3 4 5 6 7 8 9 10 11 12 roll add add add add add add add add add add }", 0, 1),
    ("{ 3 copy 3 copy pop pop pop pop pop }", 3, 4),
    ("{ 1 2 3 4 4 1 roll 4 -3 roll 4 0 roll 4 6 roll pop pop pop }", 0, 1),
    ("{ 0 index 1 index 2 index pop pop pop }", 2, 2),
    ("{ dup dup mul mul 2147483647 cvr mul cvi 2 idiv }", 1, 1),
    ("{ 65535 dup mul 65535 mul exch pop }", 1, 1),
    ("{ -2147483648 abs exch -2147483648 neg add }", 1, 2),
    ("{ 1.5 cvi exch 2.5 truncate add }", 1, 2),
    ("{ true not {1} if 1 }", 0, 1),
    ("{ 1 eq true eq {1} {2} ifelse }", 1, 1),
    ("{ 0 ne exch 1 ne or {3} {4} ifelse }", 2, 1),
    ("{ 5 not 7 xor exch pop }", 1, 1),
    ("{ % comment\n 1 add % more\n }", 1, 1),
];

#[test]
fn listed_programs_match_the_previous_interpreter() {
    let mut rng = Rng::new(11);
    for &(program, m, n) in PROGRAMS {
        let parameters = parameters(program, m, n);
        let pair = pair(&parameters);
        let mut compared = 0;
        for input in inputs(&mut rng, &parameters.domain, 40) {
            compared += usize::from(compare(&pair, &input, program));
        }
        assert!(compared > 0, "{program}");
    }
}

#[test]
fn previous_unit_test_values_hold() {
    for (program, expected) in [
        ("{ 2 3 add 4 mul }", 20.0),
        ("{ -6.5 round }", -6.0),
        ("{ 90 sin }", 1.0),
        ("{ 1 2 lt {4} {9} ifelse }", 4.0),
        ("{ 1 2 2 copy add add add }", 6.0),
        ("{ 1 2 3 3 -1 roll pop pop }", 2.0),
        ("{ -1 -1 bitshift }", 2_147_483_647.0),
        ("{ 2147483647 1 add }", 2_147_483_648.0),
        ("{ false {1 0 div} if 7 }", 7.0),
        ("{ 10 3 idiv 10 3 mod add }", 4.0),
        ("{ -2147483648 -1 mod }", 0.0),
        ("{ 3 cvr 2 exp sqrt }", 3.0),
        ("{ true false or not {0} {5} ifelse }", 5.0),
    ] {
        let mut function = parameters(program, 0, 1);
        function.range = vec![[-1e12, 1e12]];
        let function = Function::calculator(function, &Limits::default()).unwrap();
        let mut out = [0.0];
        function.evaluate(&[0.0], &mut out);
        assert!((out[0] - expected).abs() < 1e-8, "{program}: {}", out[0]);
    }
}

#[test]
fn arithmetic_faults_have_defined_results() {
    for (program, expected) in [
        ("{ 1 0 div }", f64::MAX),
        ("{ -1 0 div }", -f64::MAX),
        ("{ 0 0 div }", 0.0),
        ("{ -1 sqrt }", 0.0),
        ("{ 0 ln }", -f64::MAX),
        ("{ -1 log }", 0.0),
        ("{ 0 0 atan }", 0.0),
        ("{ 0 -0.0 atan }", 0.0),
        ("{ 1 0 idiv }", 0.0),
        ("{ 1 0 mod }", 0.0),
        ("{ -2147483648 -1 idiv }", 2_147_483_647.0),
        ("{ 10 300 exp cvi }", 2_147_483_647.0),
        ("{ 10 300 exp neg cvi }", -2_147_483_648.0),
        ("{ 10 300 exp dup mul }", f64::MAX),
        ("{ 10 300 exp dup mul 0 mul }", 0.0),
        ("{ 10 300 exp cvi 1 add 2 idiv }", 1_073_741_823.0),
        ("{ 10 300 exp cvi 1 add not }", -2_147_483_648.0),
        ("{ -2 0.5 exp }", 0.0),
    ] {
        let mut function = parameters(program, 0, 1);
        function.range = vec![[-f64::MAX, f64::MAX]];
        let function = Function::calculator(function, &Limits::default()).unwrap();
        let mut out = [f64::NAN];
        function.evaluate(&[0.0], &mut out);
        assert_eq!(out[0].to_bits(), expected.to_bits(), "{program}: {}", out[0]);
    }
}

#[test]
fn malformed_programs_fail_with_the_previous_error_kinds() {
    let limits = Limits::default();
    // Parse errors: the previous parser rejects them with the same kind.
    for program in [
        "{ file }",
        "{ {1} dup }",
        "{ 1e3 }",
        "{16#ff}",
        "1 2 add",
        "{ 1 2 add",
        "{ 1 } 2",
        "{ if }",
        "{ {1} {2} if }",
        "{ pop (secret) file }",
    ] {
        let parameters = parameters(program, 1, 1);
        let old = Oracle::calculator(&parameters, &limits).unwrap_err();
        let new = Function::calculator(parameters, &limits).unwrap_err();
        assert_eq!(new.kind, old.kind, "{program}");
        assert_eq!(new.message, old.message, "{program}");
    }
    // Faults that every previous evaluation hit are now found statically.
    for (program, inputs, message) in [
        ("{ pop }", 0, "calculator stack underflow"),
        ("{ 1 -1 copy }", 0, "negative calculator copy"),
        ("{ 3 copy }", 1, "calculator copy underflow"),
        ("{ 2 index }", 1, "calculator index underflow"),
        ("{ 3 1 roll }", 1, "calculator roll underflow"),
        ("{ -1 1 roll }", 1, "negative calculator roll"),
        ("{ true 1 and }", 0, "mismatched calculator logical operands"),
        ("{ 1.5 not }", 0, "invalid calculator logical operand"),
        ("{ true 1 add }", 0, "calculator requires numeric operand"),
        ("{ 1.5 2 idiv }", 0, "calculator requires integer"),
        ("{ 1 {2} if }", 0, "calculator condition must be boolean"),
        ("{ 1 2 }", 0, "wrong calculator output count"),
        ("{ true }", 0, "calculator requires numeric operand"),
        ("{ 1.5 copy }", 0, "calculator requires integer"),
    ] {
        let parameters = parameters(program, inputs, 1);
        let error = Function::calculator(parameters.clone(), &limits).unwrap_err();
        assert_eq!((error.kind, error.message), (ErrorKind::Syntax, message), "{program}");
        assert!(matches!(error.position, Position::Buffer(_)), "{program}");
        let old = Oracle::calculator(&parameters, &limits).unwrap();
        let mut out = [0.0];
        let old = old
            .evaluate_into(&vec![0.5; inputs.max(1)], &mut out, &mut 0)
            .unwrap_err();
        assert_eq!(old.kind, ErrorKind::Syntax, "{program}");
    }
}

#[test]
fn data_dependent_programs_are_not_validated() {
    for program in [
        "{ cvi copy }",
        "{ cvi index }",
        "{ true exch 1 exch 2 exch cvi roll pop pop 1 }",
        "{ 0.5 gt { 1 2 } { 1 } ifelse pop }",
        "{ 0.5 gt { true } { 1 } ifelse pop 1 }",
    ] {
        let error = Function::calculator(parameters(program, 1, 1), &Limits::default()).unwrap_err();
        assert_eq!(error.feature(), Some(crate::features::CALCULATOR), "{program}");
    }
    // A roll of operands of one kind is valid whatever its shift.
    assert!(
        Function::calculator(parameters("{ 1 2 3 3 index 3 exch cvi roll add add add }", 1, 1), &Limits::default())
            .is_ok()
    );
}

#[test]
fn limits_bound_nesting_size_and_stack_depth() {
    let deep = format!("{{ {} 1 {} }}", "true {".repeat(70), "} if".repeat(70));
    assert_eq!(
        Function::calculator(parameters(&deep, 0, 1), &Limits::default()).unwrap_err().kind,
        ErrorKind::Limit
    );
    // Annex C: at most 100 operands.
    let wide = format!("{{ {} {} }}", "1 ".repeat(101), "add ".repeat(100));
    assert_eq!(
        Function::calculator(parameters(&wide, 0, 1), &Limits::default()).unwrap_err().kind,
        ErrorKind::Limit
    );
    let fits = format!("{{ {} {} }}", "1 ".repeat(100), "add ".repeat(99));
    assert_eq!(evaluate(&fits, &[], 1), [100.0]);
    let limits = Limits {
        value_nodes: 3,
        ..Limits::default()
    };
    assert_eq!(
        Function::calculator(parameters("{ 1 2 add 3 add }", 0, 1), &limits).unwrap_err().kind,
        ErrorKind::Limit
    );
}

#[test]
fn cost_bounds_the_previous_work_on_every_path() {
    let function = Function::calculator(parameters("{ 1 2 add 3 mul }", 0, 1), &Limits::default())
        .unwrap();
    // The dummy input's pop, five instructions and the call.
    assert_eq!(function.cost(), 7);
    // The longer branch, the conditional itself and the moved values.
    let function = Function::calculator(
        parameters("{ 0.5 gt { 1 1 add } { 2 } ifelse dup dup 3 copy pop pop pop pop pop }", 1, 1),
        &Limits::default(),
    )
    .unwrap();
    // 0.5, gt, the conditional and its longer branch, dup, dup, 3, copy and
    // the three values it moves, five pops, and the call itself.
    assert_eq!(function.cost(), 1 + 1 + (1 + 3) + 1 + 1 + 1 + (1 + 3) + 5 + 1);
}

/// A random program that keeps track of operand kinds well enough that
/// most programs are valid.
#[expect(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "one table of small random choices"
)]
fn random_program(rng: &mut Rng, inputs: usize, outputs: usize) -> String {
    let mut text = String::from("{");
    let mut kinds = vec![false; inputs]; // true for booleans
    let push = |text: &mut String, token: &str| {
        text.push(' ');
        text.push_str(token);
    };
    for _ in 0..rng.below(24) {
        let numbers = kinds.iter().rev().take_while(|&&b| !b).count();
        let choice = rng.below(12);
        match choice {
            0 => {
                let value = if rng.chance(50) {
                    format!("{}", rng.below(11) as i32 - 5)
                } else {
                    format!("{:.3}", rng.range(-3.0, 3.0))
                };
                push(&mut text, &value);
                kinds.push(false);
            }
            1 if numbers >= 1 => {
                const UNARY: &[&str] = &[
                    "abs", "neg", "ceiling", "floor", "round", "truncate", "cvr", "sqrt", "sin",
                    "cos", "cvi", "ln", "log",
                ];
                push(&mut text, UNARY[rng.below(UNARY.len())]);
            }
            2 if numbers >= 2 => {
                const BINARY: &[&str] = &["add", "sub", "mul", "div", "exp", "atan"];
                push(&mut text, BINARY[rng.below(BINARY.len())]);
                kinds.pop();
            }
            3 if numbers >= 2 => {
                const COMPARE: &[&str] = &["eq", "ne", "ge", "gt", "le", "lt"];
                push(&mut text, COMPARE[rng.below(COMPARE.len())]);
                kinds.pop();
                kinds.pop();
                kinds.push(true);
            }
            4 if kinds.last() == Some(&true) && kinds.len() >= 2 && kinds[kinds.len() - 2] => {
                push(&mut text, ["and", "or", "xor"][rng.below(3)]);
                kinds.pop();
            }
            5 if numbers >= 2 => {
                push(&mut text, "cvi exch cvi");
                push(&mut text, ["idiv", "mod", "and", "or", "xor", "bitshift"][rng.below(6)]);
                kinds.pop();
            }
            6 if kinds.last() == Some(&true) => {
                // A conditional whose branches leave one number each.
                kinds.pop();
                let a = format!("{:.2}", rng.range(-2.0, 2.0));
                let branch = if numbers_below(&kinds) >= 1 && rng.chance(50) {
                    ["{ 2 mul } { neg }", "{ abs } { 1 add }"][rng.below(2)].to_string()
                } else {
                    kinds.push(false);
                    format!("{{ {a} }} {{ {} }}", rng.below(5))
                };
                push(&mut text, &branch);
                push(&mut text, "ifelse");
            }
            7 if !kinds.is_empty() => {
                push(&mut text, "dup");
                kinds.push(*kinds.last().unwrap_or(&false));
            }
            8 if kinds.len() >= 2 => {
                push(&mut text, "exch");
                let len = kinds.len();
                kinds.swap(len - 1, len - 2);
            }
            9 if !kinds.is_empty() => {
                let n = 1 + rng.below(kinds.len().min(4));
                let j = rng.below(7) as i32 - 3;
                push(&mut text, &format!("{n} {j} roll"));
                let len = kinds.len();
                kinds[len - n..].rotate_right(j.rem_euclid(n as i32) as usize);
            }
            10 if !kinds.is_empty() && kinds.len() < 40 => {
                let n = 1 + rng.below(kinds.len().min(3));
                push(&mut text, &format!("{n} copy"));
                let len = kinds.len();
                kinds.extend_from_within(len - n..);
            }
            11 if !kinds.is_empty() && kinds.len() < 40 => {
                let n = rng.below(kinds.len());
                push(&mut text, &format!("{n} index"));
                let kind = kinds[kinds.len() - 1 - n];
                kinds.push(kind);
            }
            _ => {}
        }
    }
    // Leave exactly `outputs` numbers.
    while let Some(&top) = kinds.last() {
        if kinds.len() <= outputs && !top {
            break;
        }
        if top {
            push(&mut text, "{ 1 } { 0 } ifelse");
            kinds.pop();
            kinds.push(false);
        } else {
            push(&mut text, "pop");
            kinds.pop();
        }
    }
    while kinds.len() < outputs {
        push(&mut text, "0.5");
        kinds.push(false);
    }
    if kinds.iter().any(|&b| b) {
        return "{ bad }".into();
    }
    text.push_str(" }");
    text
}

fn numbers_below(kinds: &[bool]) -> usize {
    kinds.iter().rev().take_while(|&&b| !b).count()
}

#[test]
fn random_programs_validate_and_match_the_previous_interpreter() {
    let mut rng = Rng::new(0xca1c);
    let (mut valid, mut compared) = (0, 0);
    for case in 0..6000 {
        let m = rng.below(4);
        let n = 1 + rng.below(3);
        let program = random_program(&mut rng, m, n);
        let parameters = parameters(&program, m, n);
        let new = Function::calculator(parameters.clone(), &Limits::default());
        let old = Oracle::calculator(&parameters, &Limits::default());
        let Ok(new) = new else {
            let error = new.unwrap_err();
            // Only programs the old parser rejected, or programs whose every
            // previous run fails, may be rejected.
            if let Ok(old) = old {
                for input in inputs(&mut rng, &parameters.domain, 16) {
                    let mut out = vec![0.0; n];
                    assert!(
                        old.evaluate_into(&input, &mut out, &mut 0).is_err(),
                        "{case}: {program} rejected ({error}) but ran"
                    );
                }
            }
            continue;
        };
        valid += 1;
        let pair = Pair {
            new: Arc::new(new),
            old: old.unwrap(),
        };
        for input in inputs(&mut rng, &parameters.domain, 16) {
            compared += usize::from(compare(&pair, &input, &format!("{case}: {program}")));
        }
    }
    assert!(valid > 4000 && compared > 40_000, "{valid} {compared}");
}

#[test]
fn every_corpus_calculator_validates_as_the_previous_interpreter_ran() {
    let root = pagura_testkit::workspace_root().join("corpus/generated");
    let mut stack = vec![root];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "pdf") {
                files.push(path);
            }
        }
    }
    files.sort();
    let (mut accepted, mut rejected) = (0, 0);
    let mut rng = Rng::new(4);
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        if !bytes.windows(13).any(|w| w == b"/FunctionType") {
            continue;
        }
        let Ok(document) =
            pagura_document::Document::open(bytes, &pagura_document::OpenOptions::default())
        else {
            continue;
        };
        for id in document.object_ids().collect::<Vec<_>>() {
            let Some(parameters) = calculator_object(&document, id) else {
                continue;
            };
            let context = format!("{} {id:?}", path.display());
            let limits = Limits::default();
            let old = Oracle::calculator(&parameters, &limits);
            match (Function::calculator(parameters.clone(), &limits), old) {
                (Ok(new), Ok(old)) => {
                    accepted += 1;
                    let pair = Pair {
                        new: Arc::new(new),
                        old,
                    };
                    for input in inputs(&mut rng, &parameters.domain, 64) {
                        compare(&pair, &input, &context);
                    }
                }
                (Err(new), Err(old)) => {
                    rejected += 1;
                    assert_eq!(new.kind, old.kind, "{context}");
                }
                (Err(new), Ok(old)) => {
                    // Rejected only if every previous run failed.
                    rejected += 1;
                    for input in inputs(&mut rng, &parameters.domain, 64) {
                        let mut out = vec![0.0; parameters.range.len()];
                        assert!(
                            old.evaluate_into(&input, &mut out, &mut 0).is_err(),
                            "{context}: {new}"
                        );
                    }
                }
                (Ok(_), Err(old)) => panic!("{context}: accepted, previously {old}"),
            }
        }
    }
    assert!(accepted >= 80, "{accepted} accepted, {rejected} rejected");
}

/// The parameters of a Type 4 function stream with a valid domain and range.
fn calculator_object(
    document: &pagura_document::Document,
    id: pagura_core::ObjectId,
) -> Option<CalculatorFunction> {
    let object = document.object(id).ok()?;
    let dictionary = object.dictionary().filter(|_| object.is_stream())?;
    if dictionary.get(b"FunctionType")?.as_integer()? != 4 {
        return None;
    }
    let pairs = |key: &[u8]| -> Option<Vec<[f64; 2]>> {
        let values = document.resolve(dictionary.get(key)?).ok()?.as_array()?;
        let numbers: Option<Vec<f64>> = values.iter().map(pagura_core::Value::as_number).collect();
        let numbers = numbers?;
        let (pairs, rest) = numbers.as_chunks::<2>();
        (rest.is_empty() && !pairs.is_empty()).then(|| pairs.to_vec())
    };
    Some(CalculatorFunction {
        domain: pairs(b"Domain")?,
        range: pairs(b"Range")?,
        program: document.stream_data(object).ok()?.into_owned().into(),
    })
}

#[test]
fn the_oracle_runs_with_ample_work() {
    assert!(unlimited().operations > 1 << 40);
}
