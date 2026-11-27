use tsox::{CheckOptions, FileId, UnitContext, Worker};

// A switch without `default` whose cases cover the discriminant's whole type is exhaustive:
// the code after it is reached only through the cases, as in tsc's flow graph.

fn count(source: &str) -> usize {
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
        .filter(|d| d.severity == tsox::Severity::Error)
        .count()
}

#[test]
fn a_literal_union_switch_that_covers_every_member_returns_on_all_paths() {
    let s = "function f(x: 'a' | 'b'): number { switch (x) { case 'a': return 1; case 'b': return 2; } }";
    assert_eq!(count(s), 0);
}

#[test]
fn a_discriminated_union_and_an_enum_switch_are_exhaustive() {
    let s = "type S = { k: 'c'; r: number } | { k: 's'; w: number };
function a(x: S): number { switch (x.k) { case 'c': return x.r; case 's': return x.w; } }
enum E { A, B }
function b(e: E): number { switch (e) { case E.A: return 1; case E.B: return 2; } }";
    assert_eq!(count(s), 0);
}

#[test]
fn a_boolean_and_a_typeof_switch_are_exhaustive() {
    let s = "function a(b: boolean): number { switch (b) { case true: return 1; case false: return 2; } }
function c(x: string | number): number { switch (typeof x) { case 'string': return 1; case 'number': return 2; } }";
    assert_eq!(count(s), 0);
}

#[test]
fn a_switch_that_misses_a_member_still_reports() {
    let s = "function f(x: 'a' | 'b' | 'c'): number { switch (x) { case 'a': return 1; case 'b': return 2; } }";
    assert_eq!(count(s), 1);
}

#[test]
fn a_variable_assigned_in_every_case_of_an_exhaustive_switch_is_definitely_assigned() {
    let s = "function f(x: 'a' | 'b') { let v: number; switch (x) { case 'a': v = 1; break; case 'b': v = 2; break; } return v; }";
    assert_eq!(count(s), 0);
}

#[test]
fn a_repeated_typeof_case_narrows_to_never() {
    let s = "function f(x: string | number) { switch (typeof x) { case 'string': break; case 'string': const n: never = x; break; } }";
    assert_eq!(count(s), 0);
}
