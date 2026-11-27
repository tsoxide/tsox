use tsox::{CheckOptions, FileId, UnitContext, Worker};

// TypeScript 5.5 inferred type predicates: a function without a return type whose one returned
// expression narrows its parameter, and in the false branch leaves exactly the rest, is a guard.

fn run(source: &str) -> Vec<(usize, u32)> {
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(CheckOptions::default());
    worker
        .check_unit(&ctx)
        .expect("the unit checks")
        .diagnostics
        .iter()
        .filter(|d| d.severity == tsox::Severity::Error)
        .map(|d| {
            (
                source[..d.start as usize].matches('\n').count() + 1,
                d.code.tsc_code().unwrap_or(0),
            )
        })
        .collect()
}

#[test]
fn a_function_returning_a_typeof_test_is_a_guard_in_both_branches() {
    let s = "function isString(x: string | number) { return typeof x === 'string'; }
declare let v: string | number;
if (isString(v)) { let t: string = v; } else { let t: number = v; }";
    assert!(run(s).is_empty());
}

#[test]
fn an_arrow_const_and_a_filter_callback_are_guards() {
    let s = "const isNum = (x: number | null) => x !== null;
const a: number[] = [1, null].filter(isNum);
const b: number[] = [1, null].filter(x => x !== null);";
    assert!(run(s).is_empty());
}

#[test]
fn a_truthiness_test_is_not_a_guard_because_zero_is_a_number() {
    let s = "const a: number[] = [1, null].filter(x => !!x);";
    assert_eq!(run(s).len(), 1);
}

#[test]
fn a_declaration_mismatch_is_reported_on_the_name_not_the_initializer() {
    let s = "declare const xs: (number | null)[];
const ys: number[] =
    xs.map(x => x);";
    assert_eq!(run(s), vec![(2, 2322)]);
}
