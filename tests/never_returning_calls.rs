use tsox::{CheckOptions, FileId, UnitContext, Worker};

// A call statement to a function declared `never` ends the flow, as in tsc's flow graph: the
// function does not run off its end, and code after the call is unreachable. The declaration must
// say `never` (a `function f(): never`, or a variable or parameter typed `(..) => never`); a
// function whose `never` is only inferred does not count.

fn codes(source: &str) -> Vec<u32> {
    let options = CheckOptions {
        strict: true,
        strict_null_checks: true,
        no_implicit_returns: true,
        ..CheckOptions::default()
    };
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(options);
    worker
        .check_unit(&ctx)
        .expect("the unit checks")
        .diagnostics
        .iter()
        .filter_map(|d| d.code.tsc_code())
        .collect()
}

const FAIL: &str = "declare function fail(message?: string): never;\n";

#[test]
fn a_function_ending_in_a_never_call_does_not_run_off_its_end() {
    let s = format!("{FAIL}function f(x: number): number {{ if (x >= 0) return x; fail(); }}");
    assert!(codes(&s).is_empty());
}

#[test]
fn code_after_a_never_call_is_unreachable() {
    let s = format!("{FAIL}function f(x: string) {{ fail(); x; }}");
    assert_eq!(codes(&s), vec![7027]);
}

#[test]
fn a_never_typed_parameter_counts() {
    let s = "function f(x: number, fail: (m?: string) => never): number { if (x >= 0) return x; fail(); }";
    assert!(codes(s).is_empty());
}

#[test]
fn an_unannotated_arrow_does_not_count() {
    let s = "function f(x: number): number { const fail = (): never => { throw new Error(); }; if (x >= 0) return x; fail(); }";
    assert_eq!(codes(s), vec![2366]);
}

#[test]
fn an_optional_call_does_not_end_the_flow() {
    let s = "function f(x: number, fail?: () => never): number { if (x >= 0) return x; fail?.(); }";
    assert_eq!(codes(s), vec![2366]);
}
