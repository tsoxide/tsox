use tsox::{CheckOptions, FileId, UnitContext, Worker};

// Type guards that are methods, and assertion functions held in an annotated variable. A method is
// found by its name, so a name declared twice with different predicates narrows nothing.

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

#[test]
fn a_this_guard_method_narrows_the_receiver() {
    let s = "class G { isLead(): this is L { return true; } }
class L extends G { lead(): void {} }
function f(a: G) { if (a.isLead()) { a.lead(); } }";
    assert_eq!(count(s), 0);
}

#[test]
fn a_parameter_guard_method_narrows_the_argument() {
    let s = "class V { isStr(x: unknown): x is string { return typeof x === 'string'; } }
function f(v: V, x: unknown) { if (v.isStr(x)) { x.length; } }";
    assert_eq!(count(s), 0);
}

#[test]
fn an_asserting_method_narrows_after_the_call() {
    let s = "class V { assert(x: unknown): asserts x { } }
function f(v: V, x: string | undefined) { v.assert(x); x.length; }";
    assert_eq!(count(s), 0);
}

#[test]
fn a_name_declared_with_two_meanings_narrows_nothing() {
    let s = "class A { is(): this is B { return true; } }
class B extends A { b(): void {} }
class C { is(): boolean { return true; } }
function f(a: A) { if (a.is()) { a.b(); } }";
    assert_eq!(count(s), 1);
}

#[test]
fn an_assertion_held_in_an_annotated_const_narrows() {
    let s = "const assert: (value: unknown) => asserts value = value => {};
function f(x: string | undefined) { assert(x); x.length; }";
    assert_eq!(count(s), 0);
}
