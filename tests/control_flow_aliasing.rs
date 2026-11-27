use tsox::{CheckOptions, FileId, UnitContext, Worker};

// tsc 4.4's control-flow analysis of aliased conditions and discriminants: a `const` holding a
// condition stands for it, and a name that reads a property of a variable nothing assigns to
// narrows that variable when it is tested.

fn count(source: &str) -> usize {
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(CheckOptions::default());
    worker
        .check_unit(&ctx)
        .expect("the unit checks")
        .diagnostics
        .iter()
        .filter(|d| d.severity == tsox::Severity::Error)
        .count()
}

const SHAPE: &str = "type S = { kind: 'a'; a: string } | { kind: 'b'; b: number };\n";

#[test]
fn an_or_of_two_aliases_narrows_the_else_branch_by_both() {
    let s = "function f(x: string | number | boolean) {
  const isString = typeof x === 'string';
  const isNumber = typeof x === 'number';
  if (isString || isNumber) { let t: string | number = x; } else { let t: boolean = x; }
}";
    assert_eq!(count(s), 0);
}

#[test]
fn a_property_alias_narrows_its_object_by_equality() {
    let s = format!(
        "{SHAPE}function f(o: S) {{ const kind = o.kind; if (kind === 'a') {{ o.a; }} else {{ o.b; }} }}"
    );
    assert_eq!(count(&s), 0);
}

#[test]
fn a_destructured_alias_narrows_by_equality_and_by_switch() {
    let s = format!(
        "{SHAPE}function f(o: S) {{ const {{ kind }} = o; if (kind === 'a') {{ o.a; }} else {{ o.b; }} }}
function g(o: S) {{ const {{ kind }} = o; switch (kind) {{ case 'a': o.a; break; case 'b': o.b; break; }} }}"
    );
    assert_eq!(count(&s), 0);
}

#[test]
fn a_reassigned_object_is_not_narrowed_through_its_alias() {
    let s = format!(
        "{SHAPE}function f(o: S, p: S) {{ const kind = o.kind; o = p; if (kind === 'a') {{ o.a; }} }}"
    );
    assert_eq!(count(&s), 1);
}

#[test]
fn the_alias_constant_keeps_its_own_truthiness_narrowing() {
    let s = "declare let on: boolean;
function f(w: { p?: { m(): void } }) { const p = on && w.p; if (p) { p.m(); } }";
    assert_eq!(count(s), 0);
}
