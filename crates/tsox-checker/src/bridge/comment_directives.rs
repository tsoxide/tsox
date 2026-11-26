// `// @ts-ignore` and `// @ts-expect-error`.
//
// Problem: neither directive was read, so an error the author had deliberately silenced was
// reported anyway, and an `@ts-expect-error` that silenced nothing was never reported.
// Picked: tsc's own rule (`markPrecedingCommentDirectiveLine`). An error is silenced when the
// directive is on a line above it with only blank lines and `//` comment lines between; the
// first directive met going up is the one that takes it. A directive is found in a `//` comment (after `//` or `///`, spaces allowed)
// or in the last line of a `/* */` comment (after any run of `/` and `*`).
// Cost: an `@ts-expect-error` that silences nothing (tsc's TS2578) is not reported. This checker
// misses errors tsc finds, so a directive over one of those would be called unused wrongly,
// which is an error invented for the user; the corpus showed it (a missing TS2550 or JSX error
// became a false TS2578). Parser errors are not silenced, as in tsc, but a grammar error this checker finds in a
// later pass is, which tsc also does for its checker-side grammar errors. A leading
// `@ts-nocheck` clears the checker's diagnostics.
// Now: applied to the checker's diagnostics of one file, before its syntax errors are added.

use oxc_ast::ast::{CommentKind, Program};
use oxc_span::GetSpan;

use tsox_base::diagnostics::Diagnostic;

use super::syntax_repair::OffsetMap;

struct Directive {
    line: usize,
}

fn is_directive(text: &str, multi_line: bool) -> bool {
    let mut rest = text.trim_start();
    if multi_line {
        rest = rest.trim_start_matches(['/', '*']).trim_start();
    } else {
        match rest.strip_prefix("///").or_else(|| rest.strip_prefix("//")) {
            Some(after) => rest = after.trim_start(),
            None => return false,
        }
    }
    rest.strip_prefix('@')
        .is_some_and(|name| name.starts_with("ts-expect-error") || name.starts_with("ts-ignore"))
}

pub fn apply(
    program: &Program<'_>,
    original: &str,
    offsets: Option<&OffsetMap>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let source = program.source_text;
    // `// @ts-nocheck` among the comments before the first statement: tsc reports none of the
    // checker's diagnostics for the file (and so no unused `@ts-expect-error` either).
    let first_statement = program.body.first().map_or(u32::MAX, |s| s.span().start);
    let nocheck = program.comments.iter().any(|comment| {
        matches!(comment.kind, CommentKind::Line)
            && comment.span.end <= first_statement
            && source
                .get(comment.span.start as usize..comment.span.end as usize)
                .is_some_and(|text| {
                    let rest = text.strip_prefix("///").or_else(|| text.strip_prefix("//"));
                    rest.is_some_and(|rest| rest.trim() == "@ts-nocheck")
                })
    });
    if nocheck {
        diagnostics.clear();
        return;
    }
    let mut directives: Vec<Directive> = Vec::new();
    for comment in &program.comments {
        let (start, end) = (comment.span.start as usize, comment.span.end as usize);
        let Some(text) = source.get(start..end) else {
            continue;
        };
        let multi_line = !matches!(comment.kind, CommentKind::Line);
        // A block comment is judged by its last line.
        let (tested, tested_start) = if multi_line {
            match text.rfind('\n') {
                Some(at) => (&text[at + 1..], start + at + 1),
                None => (text, start),
            }
        } else {
            (text, start)
        };
        if !is_directive(tested, multi_line) {
            continue;
        }
        let to_original = |offset: usize, is_end: bool| match offsets {
            Some(map) => map.to_original(offset as u32, is_end),
            None => offset as u32,
        };
        let line_start = to_original(tested_start, false);
        let line = original[..(line_start as usize).min(original.len())]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        directives.push(Directive { line });
    }
    if directives.is_empty() {
        return;
    }

    let mut line_starts: Vec<usize> = vec![0];
    for (index, byte) in original.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(index + 1);
        }
    }
    let line_text = |line: usize| -> &str {
        let from = line_starts[line];
        let to = line_starts.get(line + 1).copied().unwrap_or(original.len());
        original[from..to].trim()
    };

    diagnostics.retain(|diagnostic| {
        let at = (diagnostic.start as usize).min(original.len());
        let mut line = line_starts.partition_point(|&start| start <= at);
        // `line` is the 1-based number of the error's line; the lines above it are 0-based
        // indexes below `line - 1`.
        line = line.saturating_sub(1);
        while line > 0 {
            line -= 1;
            if directives.iter().any(|d| d.line == line) {
                return false;
            }
            let text = line_text(line);
            if !text.is_empty() && !text.starts_with("//") {
                break;
            }
        }
        true
    });
}
