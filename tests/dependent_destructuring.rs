use tsox::{CheckOptions, FileId, UnitContext, Worker};

// Destructured bindings that stay tied to each other (tsc 4.6): testing one narrows the others,
// for an object pattern, a tuple pattern, and a generic parameter constrained to the union. A
// union of tuples is also an ordinary destructuring source, with `undefined` where a member is
// shorter.

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

const ACTION: &str =
    "type Action = { kind: 'A'; payload: number } | { kind: 'B'; payload: string };\n";

#[test]
fn a_union_of_tuples_destructures_by_position() {
    let s = "function f([x, y]: [1, 2] | [3, 4] | [5]) { const n: 2 | 4 | undefined = y; const m: 1 | 3 | 5 = x; }";
    assert_eq!(count(s), 0);
}

#[test]
fn a_tuple_pattern_ties_its_names_together() {
    let s = "function f([kind, payload]: ['A', number] | ['B', string]) {
  if (kind === 'A') { payload.toFixed(); } else { payload.toUpperCase(); }
}";
    assert_eq!(count(s), 0);
}

#[test]
fn an_object_pattern_over_a_generic_constraint_ties_its_names() {
    let s = format!(
        "{ACTION}function f<T extends Action>({{ kind, payload }}: T) {{ if (kind === 'A') {{ payload.toFixed(); }} }}"
    );
    assert_eq!(count(&s), 0);
}

#[test]
fn a_sibling_keeps_what_it_was_already_narrowed_to() {
    let s = "type A = { kind: 'A'; payload: number | undefined } | { kind: 'B'; payload: string | undefined };
function f({ kind, payload }: A) { if (payload) { if (kind === 'A') { payload.toFixed(); } } }";
    assert_eq!(count(s), 0);
}

#[test]
fn an_untested_binding_is_still_the_whole_union() {
    let s = format!("{ACTION}function f({{ kind, payload }}: Action) {{ payload.toFixed(); }}");
    assert_eq!(count(&s), 1);
}
