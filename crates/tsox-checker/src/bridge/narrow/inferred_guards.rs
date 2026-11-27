// Owns: inferred type predicates (TypeScript 5.5): `x => x !== null` and `function f(x: A | B) {
// return typeof x === "string"; }` are guards `x is ...` without being written as one. Does not own
// written predicates: see `predicate.rs`.
//
// tsc infers the predicate when the one returned expression narrows the parameter in both
// directions that are exact complements: the true branch holds type `T`, and the false branch holds
// precisely what is left of the declared type once `T` is taken away. `!!x` on `number | null` is
// not one (`0` is a number that makes it false), so a callback written that way still gives
// `(number | null)[]`.

use super::*;
use oxc_ast::ast::{
    BindingPattern, Expression as Ex, FormalParameters, FunctionBody, Statement, VariableDeclarator,
};
use oxc_ast_visit::{Visit, walk};

/// The expression a function returns when that is all it does: a block with exactly one
/// `return expr;`.
pub fn returned_expression<'a, 'b>(body: &'b FunctionBody<'a>) -> Option<&'b Ex<'a>> {
    let [Statement::ReturnStatement(statement)] = body.statements.as_slice() else {
        return None;
    };
    statement.argument.as_ref()
}

/// The same for an arrow: a concise body is the returned expression itself.
pub fn arrow_returned<'a, 'b>(body: &'b oxc_ast::ast::ArrowFunctionBody<'a>) -> Option<&'b Ex<'a>> {
    match body {
        oxc_ast::ast::ArrowFunctionBody::FunctionBody(block) => returned_expression(block),
        other => other.as_expression(),
    }
}

fn same_members(arena: &TypeArena, a: &[TypeId], b: &[TypeId]) -> bool {
    use crate::subtyping::is_subtype;
    a.iter()
        .all(|&x| b.iter().any(|&y| is_subtype(arena, x, y)))
        && b.iter()
            .all(|&y| a.iter().any(|&x| is_subtype(arena, y, x)))
}

fn infer(
    symbol: SymbolId,
    declared: TypeId,
    expression: &Ex<'_>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    if scoping.symbol_is_mutated(symbol) {
        return None;
    }
    let saved = std::mem::replace(&mut ctx.narrow, NarrowState::single(symbol, declared));
    let (when_true, when_false) = narrow_condition(expression, scoping, ctx);
    ctx.narrow = saved;
    let claimed = when_true.get(symbol)?;
    let when_false = when_false.get(symbol).unwrap_or(declared);
    let all = union_members(&ctx.arena, declared).unwrap_or_else(|| vec![declared]);
    let left: Vec<TypeId> = all
        .iter()
        .copied()
        .filter(|&m| !crate::subtyping::is_subtype(&ctx.arena, m, claimed))
        .collect();
    if left.is_empty()
        || left.len() == all.len()
        || matches!(ctx.arena.get(claimed), Type::Never)
        || !crate::subtyping::is_subtype(&ctx.arena, claimed, declared)
    {
        return None;
    }
    let false_members = union_members(&ctx.arena, when_false).unwrap_or_else(|| vec![when_false]);
    same_members(&ctx.arena, &left, &false_members).then_some(claimed)
}

/// A callback written in place, whose parameter type comes from where it is passed
/// (`xs.filter(x => x !== null)`).
pub fn infer_inline_guard(
    params: &FormalParameters<'_>,
    returned: Option<&Ex<'_>>,
    written_return: bool,
    declared: TypeId,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> Option<TypeId> {
    if written_return || params.items.len() != 1 || params.rest.is_some() {
        return None;
    }
    let BindingPattern::BindingIdentifier(id) = &params.items[0].pattern else {
        return None;
    };
    let symbol = id.symbol_id.get()?;
    infer(symbol, declared, returned?, scoping, ctx)
}

struct Scan<'c, 'a, 'b, 's> {
    ctx: &'c mut CheckContext<'a, 'b>,
    scoping: &'s Scoping,
}

impl Scan<'_, '_, '_, '_> {
    fn note(
        &mut self,
        function: SymbolId,
        params: &FormalParameters<'_>,
        returned: Option<&Ex<'_>>,
        written_return: bool,
    ) {
        if written_return || params.items.len() != 1 || params.rest.is_some() {
            return;
        }
        let parameter = &params.items[0];
        let BindingPattern::BindingIdentifier(id) = &parameter.pattern else {
            return;
        };
        let (Some(symbol), Some(annotation)) = (id.symbol_id.get(), &parameter.type_annotation)
        else {
            return;
        };
        let Some(declared) = crate::type_annotation::resolve_type_annotation(
            annotation,
            &mut self.ctx.namespace,
            &mut self.ctx.arena,
        ) else {
            return;
        };
        let Some(expression) = returned else {
            return;
        };
        if let Some(claimed) = infer(symbol, declared, expression, self.scoping, self.ctx) {
            self.ctx.type_predicates.insert(
                function,
                crate::bridge::context::TypePredicate {
                    param_index: 0,
                    type_id: Some(claimed),
                    asserts: false,
                },
            );
        }
    }
}

impl<'a> Visit<'a> for Scan<'_, '_, '_, '_> {
    fn visit_function(
        &mut self,
        function: &oxc_ast::ast::Function<'a>,
        flags: oxc_semantic::ScopeFlags,
    ) {
        if let (Some(id), Some(body)) = (&function.id, &function.body)
            && let Some(symbol) = id.symbol_id.get()
            && function.type_parameters.is_none()
        {
            self.note(
                symbol,
                &function.params,
                returned_expression(body),
                function.return_type.is_some(),
            );
        }
        walk::walk_function(self, function, flags);
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        if let (BindingPattern::BindingIdentifier(id), Some(init)) =
            (&declarator.id, &declarator.init)
            && declarator.type_annotation.is_none()
            && let Some(symbol) = id.symbol_id.get()
            && let Ex::ArrowFunctionExpression(arrow) = init
            && arrow.type_parameters.is_none()
        {
            self.note(
                symbol,
                &arrow.params,
                arrow_returned(&arrow.body),
                arrow.return_type.is_some(),
            );
        }
        walk::walk_variable_declarator(self, declarator);
    }
}

/// Records the inferred guards of the functions in a file whose parameter is annotated, before the
/// bodies are checked.
pub fn collect_inferred_guards<'ast>(
    program: &'ast oxc_ast::ast::Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
) {
    Scan { ctx, scoping }.visit_program(program);
}
