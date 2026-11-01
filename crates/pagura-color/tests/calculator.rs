#![forbid(unsafe_code)]
//! Independent endpoint contracts for statically proved branch alternatives.

use pagura_color::{CalculatorFunction, Function};
use pagura_core::{ErrorKind, Limits};
use std::sync::Arc;

const PIECEWISE: &str =
    "{dup 0 ge {dup 1 sub abs 0.005 lt {pop 0.934 0.1935 0.1406 -5 } if } if 0 ge {1 0 0 } if }";

fn function(program: &str, outputs: usize) -> pagura_core::Result<Function> {
    Function::calculator(
        CalculatorFunction {
            domain: vec![[0.0, 1.0]],
            range: vec![[0.0, 1.0]; outputs],
            program: Arc::from(program.as_bytes()),
        },
        &Limits::default(),
    )
}

#[test]
fn piecewise_endpoints_and_clipping_have_independent_colors() {
    let function = function(PIECEWISE, 3).unwrap();
    // The special-color path executes nineteen calculator instructions;
    // the red path executes seventeen. One more unit accounts for the call.
    assert_eq!(function.cost(), 20);
    let threshold = 0.995_f64;
    // The endpoint table is authored directly, without executing a second
    // interpreter or deriving expected colors from the program under test.
    for (input, expected) in [
        (f64::NEG_INFINITY, [1.0, 0.0, 0.0]),
        (-1.0, [1.0, 0.0, 0.0]),
        (-0.0, [1.0, 0.0, 0.0]),
        (0.0, [1.0, 0.0, 0.0]),
        (0.5, [1.0, 0.0, 0.0]),
        (threshold.next_down(), [1.0, 0.0, 0.0]),
        (threshold, [1.0, 0.0, 0.0]),
        (threshold.next_up(), [0.934, 0.1935, 0.1406]),
        (1.0, [0.934, 0.1935, 0.1406]),
        (2.0, [0.934, 0.1935, 0.1406]),
        (f64::INFINITY, [0.934, 0.1935, 0.1406]),
        (f64::NAN, [1.0, 0.0, 0.0]),
    ] {
        let mut actual = [0.0; 3];
        function.evaluate(&[input], &mut actual);
        assert_eq!(actual, expected, "input={input:?}");
    }
    let mut missing = [0.0; 3];
    function.evaluate(&[], &mut missing);
    assert_eq!(missing, [1.0, 0.0, 0.0]);
}

#[test]
fn all_ordered_comparisons_use_only_clamped_finite_domain_facts() {
    for (operator, domain) in [
        ("ge", [0.0, 1.0]),
        ("le", [-1.0, 0.0]),
        ("gt", [0.25, 1.0]),
        ("lt", [-1.0, -0.25]),
    ] {
        let program = format!("{{dup 0 {operator} {{pop 0.2 0.4 0.6}} if}}");
        let function = Function::calculator(
            CalculatorFunction {
                domain: vec![domain],
                range: vec![[0.0, 1.0]; 3],
                program: Arc::from(program.as_bytes()),
            },
            &Limits::default(),
        )
        .unwrap();
        for input in [-2.0, -0.0, 0.0, 0.25, 0.5, 1.0, 2.0, f64::NAN] {
            let mut actual = [0.0; 3];
            function.evaluate(&[input], &mut actual);
            assert_eq!(actual, [0.2, 0.4, 0.6], "{operator}, input={input:?}");
        }
    }
}

#[test]
fn arithmetic_never_reuses_an_input_interval_as_a_result_interval() {
    for (program, domain) in [
        ("{dup 0 ge {pop 0.5} if neg 0 ge {1 0 0} if}", [0.0, 1.0]),
        ("{dup 0 le {pop -0.5} if abs 0 le {1 0 0} if}", [-1.0, 0.0]),
        ("{dup 0 ge {pop 0.5} if floor 0 gt {1 0 0} if}", [0.0, 1.0]),
        (
            "{dup 0 le {pop -0.5} if ceiling 0 lt {1 0 0} if}",
            [-1.0, 0.0],
        ),
        ("{dup 0 ge {pop 0.1} if round 0 gt {1 0 0} if}", [0.0, 1.0]),
        (
            "{dup 0 ge {pop 0.5} if truncate 0 gt {1 0 0} if}",
            [0.0, 1.0],
        ),
    ] {
        let error = Function::calculator(
            CalculatorFunction {
                domain: vec![domain],
                range: vec![[0.0, 1.0]; 3],
                program: Arc::from(program.as_bytes()),
            },
            &Limits::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(pagura_color::features::CALCULATOR),
            "{program}"
        );
        assert_eq!(error.feature(), Some(pagura_color::features::CALCULATOR));
    }
}

#[test]
fn varying_depths_rejoin_only_after_their_continuations() {
    let function = function("{dup 0.5 gt {pop 0.2 0.4 -1} if 0 ge {0.6 0.8} if}", 2).unwrap();
    for (input, expected) in [(0.0, [0.6, 0.8]), (0.5, [0.6, 0.8]), (1.0, [0.2, 0.4])] {
        let mut actual = [0.0; 2];
        function.evaluate(&[input], &mut actual);
        assert_eq!(actual, expected);
    }
}

#[test]
fn existing_fast_proofs_keep_their_outputs_and_costs() {
    for (program, cost, endpoints) in [
        ("{1 exch sub}", 4, [1.0, 0.0]),
        ("{0.5 gt {0.2} {0.8} ifelse}", 5, [0.8, 0.2]),
        ("{pop true {1 4 div} {false 9 sub} ifelse}", 7, [0.25, 0.25]),
    ] {
        let function = function(program, 1).unwrap();
        assert_eq!(function.cost(), cost, "{program}");
        for (input, expected) in [0.0, 1.0].into_iter().zip(endpoints) {
            let mut actual = [0.0];
            function.evaluate(&[input], &mut actual);
            assert_eq!(actual, [expected], "{program}");
        }
    }
}

#[test]
fn merged_prefix_work_includes_copy_and_roll_on_the_longest_path() {
    let function = function(
        "{dup 0.5 gt {pop 0.25 dup dup 3 copy 6 1 roll pop pop pop pop pop -1} if 0 ge {0.75} if}",
        1,
    )
    .unwrap();
    // Four prefix instructions; fifteen instructions plus three copied
    // and six rolled operands on the long branch; three suffix instructions;
    // and one function call. The short path must not replace that prefix cost.
    assert_eq!(function.cost(), 4 + 15 + 3 + 6 + 3 + 1);
    for (input, expected) in [(0.0, 0.75), (1.0, 0.25)] {
        let mut actual = [0.0];
        function.evaluate(&[input], &mut actual);
        assert_eq!(actual, [expected]);
    }
}

#[test]
fn unproved_paths_keep_the_original_explicit_refusals() {
    for (name, outputs) in [
        ("three-depths", 1),
        ("branch-type-mismatch", 3),
        ("arithmetic-facts-unproved", 3),
    ] {
        let bytes = pagura_testkit::fixture("calculator_branches", &format!("{name}.ps"));
        let program = std::str::from_utf8(&bytes).unwrap();
        let error = function(program, outputs).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(pagura_color::features::CALCULATOR),
            "{name}"
        );
        assert_eq!(error.feature(), Some(pagura_color::features::CALCULATOR));
    }
    for program in [
        // Three concurrent depths exceed this proof's declared two-state scope.
        "{dup 0.25 lt {pop 0.1 1} {dup 0.75 lt {pop 0.2 0.3 2} {pop 0.4 0.5 0.6 3} ifelse} ifelse dup 1 eq {pop} {dup 2 eq {pop pop} {pop pop pop} ifelse} ifelse}",
        "{0.5 gt {1 2} {1} ifelse pop}",
        "{0.5 gt {true} {1} ifelse pop 1}",
        "{cvi copy}",
    ] {
        let error = function(program, 1).unwrap_err();
        assert_eq!(
            error.kind,
            ErrorKind::Unsupported(pagura_color::features::CALCULATOR),
            "{program}"
        );
        assert_eq!(error.feature(), Some(pagura_color::features::CALCULATOR));
    }
    // Removing the nonnegative Domain fact exposes a genuine empty-output path.
    let error = Function::calculator(
        CalculatorFunction {
            domain: vec![[-1.0, 1.0]],
            range: vec![[0.0, 1.0]; 3],
            program: Arc::from(PIECEWISE.as_bytes()),
        },
        &Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        error.kind,
        ErrorKind::Unsupported(pagura_color::features::CALCULATOR)
    );
    assert_eq!(error.feature(), Some(pagura_color::features::CALCULATOR));
}

#[test]
fn a_valid_alternative_can_reach_but_never_exceed_stack_one_hundred() {
    let program = |zeros| {
        format!(
            "{{dup 0.5 gt {{pop 0.25 {}{} -1}} if 0 ge {{0.75}} if}}",
            "0 ".repeat(zeros),
            "pop ".repeat(zeros)
        )
    };
    let function = function(&program(99), 1).unwrap();
    let mut actual = [0.0];
    function.evaluate(&[1.0], &mut actual);
    assert_eq!(actual, [0.25]);
    let error = self::function(&program(100), 1).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Limit);
}
