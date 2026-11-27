use tsox::TypeChecker;

// `// @ts-ignore` and `// @ts-expect-error` silence what the next line reports, as in tsc
// (`markPrecedingCommentDirectiveLine`): the directive is on a line above the error with only
// blank lines and `//` comment lines between. A `@ts-nocheck` before the first statement clears
// the checker's diagnostics for the file. See bridge/comment_directives.rs.

fn error_lines(source: &str) -> Vec<usize> {
    let checked = TypeChecker::new()
        .check_source(source, "a.ts")
        .expect("the file checks");
    let mut lines: Vec<usize> = checked
        .diagnostics
        .iter()
        .map(|d| source[..d.start as usize].matches('\n').count() + 1)
        .collect();
    lines.sort();
    lines
}

#[test]
fn a_directive_silences_the_error_on_the_next_code_line() {
    let source = "\
// @ts-ignore with a note
var a: number = 'nope';

// @ts-expect-error
// a comment between
var b: number = 'nope';

var c: number = 'nope';
";
    assert_eq!(error_lines(source), vec![8]);
}

#[test]
fn a_block_comment_directive_and_a_trailing_directive_work_like_tsc() {
    let source = "\
/* @ts-ignore */
var a: number = 'nope';
var b: number = 'nope'; // @ts-ignore
var c: number = 'nope';
var d: number = 'nope';
";
    // The trailing directive on line 3 covers line 4, not line 3, and a directive covers one line.
    assert_eq!(error_lines(source), vec![3, 5]);
}

#[test]
fn code_between_a_directive_and_the_error_ends_its_reach() {
    let source = "\
// @ts-ignore
const fine = 1;
var a: number = 'nope';
";
    assert_eq!(error_lines(source), vec![3]);
}

#[test]
fn a_leading_ts_nocheck_clears_the_checker_diagnostics() {
    let source = "// @ts-nocheck\nvar a: number = 'nope';\n";
    assert!(error_lines(source).is_empty());
    // After the first statement it is only a comment.
    let late = "var a: number = 'nope';\n// @ts-nocheck\n";
    assert_eq!(error_lines(late), vec![1]);
}
