use tsox::{CheckOptions, FileId, UnitContext, Worker};

// A file's own `interface Promise<T>` adds members to the built-in Promise, which otherwise has a
// fixed few (`then`, `catch`, `finally`); and `export = Name` / `export default Name` may name an
// interface or alias, which is a type and not a value read.

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
fn a_member_added_to_promise_by_the_file_is_found() {
    let s = "interface Promise<T> { done(cb: (v: T) => void): void; }
declare const p: Promise<number>;
p.done(v => v.toFixed());";
    assert_eq!(count(s), 0);
}

#[test]
fn a_then_with_a_third_callback_written_by_the_file_is_the_one_called() {
    let s = "interface Promise<T> {
  then<U>(a?: (v: T) => U, b?: (e: any) => U, c?: (p: any) => void): Promise<U>;
}
declare const p: Promise<number>;
const r = p.then(v => v, e => e, x => {});";
    assert_eq!(count(s), 0);
}

#[test]
fn a_member_the_file_did_not_write_is_still_missing() {
    let s = "interface Promise<T> { done(): void; }
declare const p: Promise<number>;
p.nope();";
    assert_eq!(count(s), 1);
}

#[test]
fn export_equals_and_export_default_may_name_a_type() {
    let s = "interface Point { x: number; }
export = Point;";
    assert_eq!(count(s), 0);
    let s = "interface Point { x: number; }
export default Point;";
    assert_eq!(count(s), 0);
}

#[test]
fn export_equals_of_an_unknown_name_is_still_reported() {
    assert_eq!(count("export = Missing;"), 1);
}
