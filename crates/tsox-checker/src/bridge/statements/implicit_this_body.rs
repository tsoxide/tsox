//! Report TS2683 (`'this' implicitly has type 'any'`) for a plain function whose body the
//! checker could not otherwise check.
//!
//! The body-check pass in `statements::functions` runs only when the function's signature
//! resolved to a type the arena can read. A function with no annotations on its parameters
//! (`function f(x, y) { this.x = x; }`) does not resolve, so its body is skipped and a `this`
//! used in it goes unreported, where tsc says TS2683 once per `this` under `noImplicitThis`.
//!
//! This visitor walks just the body of such a function, reporting each `this` that has no
//! type annotation. It does not descend into nested functions, arrows, methods or classes:
//! each of those has its own `this` semantics, so a `this` inside one is not the outer
//! function's and is not this pass's concern. Other body-level errors are left to whatever
//! does run; this pass only closes the TS2683 gap.

use oxc_ast::ast::Statement;
use oxc_ast_visit::Visit;

use super::super::context::CheckContext;

/// Walk a function body's statements and report every `this` whose type is unknown, the way
/// tsc reports TS2683 under `noImplicitThis`. Call this only for a plain function (no `this:`
/// parameter) whose body the ordinary body-check pass skipped.
///
/// The body statements are passed in (rather than the whole `Function`) so the borrow of
/// `ctx` and the borrow of the AST do not have to share a named lifetime up the call chain.
/// `CheckContext`'s first lifetime parameter is the arena's, which is independent of the
/// AST slice's lifetime here: this pass only calls `ctx.error` and `ctx.option`, neither of
/// which reads the arena-tied lifetime, so the two can be unrelated.
pub(super) fn report_implicit_this_in_unchecked_body<'ast>(
    body: &'ast [Statement<'ast>],
    ctx: &mut CheckContext<'_, '_>,
) {
    if !ctx.option(crate::check_options::OptionField::NoImplicitThis) {
        return;
    }
    let mut walker = ThisVisitor { ctx };
    for stmt in body {
        walker.visit_statement(stmt);
    }
}

struct ThisVisitor<'c, 'ctx_ast, 'src> {
    ctx: &'c mut CheckContext<'ctx_ast, 'src>,
}

impl<'ast, 'c, 'ctx_ast, 'src> Visit<'ast> for ThisVisitor<'c, 'ctx_ast, 'src> {
    fn visit_this_expression(&mut self, this: &oxc_ast::ast::ThisExpression) {
        self.ctx.error(
            crate::diagnostic_messages::messages::this_implicitly_any(),
            this.span,
        );
    }

    // Do not descend into nested scopes that have their own `this`: a function, arrow, method
    // or class body. `this` inside one of them is not the enclosing function's `this`, so it
    // is not reported here. (Arrows have no own `this`, but they read the enclosing one only
    // at call time; tsc reports TS2683 for an arrow's `this` against the nearest *function*
    // scope, which is the one this pass already reported if it was the unchecked body. Walking
    // into the arrow would re-report the same `this`, so arrows are skipped too.)
    fn visit_function(&mut self, _: &oxc_ast::ast::Function<'ast>, _: oxc_semantic::ScopeFlags) {}
    fn visit_arrow_function_expression(&mut self, _: &oxc_ast::ast::ArrowFunctionExpression<'ast>) {
    }
    fn visit_method_definition(&mut self, _: &oxc_ast::ast::MethodDefinition<'ast>) {}
    fn visit_class(&mut self, _: &oxc_ast::ast::Class<'ast>) {
        // A class body is its own scope for `this`; skip it entirely.
    }
}
