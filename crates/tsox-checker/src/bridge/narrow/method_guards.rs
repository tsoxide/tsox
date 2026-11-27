// Owns: type guards and assertions that are methods (`a.isLeader()` with `this is LeadGuard`,
// `this.assert(x)` with `asserts x`). Does not own function-declaration guards: see
// `predicate.rs`.
//
// A method is found by its name alone, so the pre-scan keeps a name only when every method,
// property and signature of that name in the file carries the same predicate; a name declared
// twice with different meanings narrows nothing, which is always safe.

use super::*;
use oxc_ast::ast::{
    Expression as Ex, MethodDefinition, PropertyDefinition, TSMethodSignature, TSPropertySignature,
    TSType, TSTypeAnnotation, TSTypePredicateName,
};
use oxc_ast_visit::{Visit, walk};

#[derive(Clone, Copy, PartialEq)]
pub enum GuardTarget {
    /// `this is T`, `asserts this [is T]`: the object the method is called on.
    This,
    /// `x is T`, `asserts x`: the argument at this position.
    Param(usize),
}

#[derive(Clone, Copy, PartialEq)]
pub struct MethodGuard {
    pub target: GuardTarget,
    pub type_id: Option<TypeId>,
    pub asserts: bool,
}

struct Scan<'c, 'a, 'b> {
    ctx: &'c mut CheckContext<'a, 'b>,
    // `None` once a name is seen twice with different meanings, or once without a predicate.
    found: std::collections::HashMap<String, Option<MethodGuard>>,
}

impl Scan<'_, '_, '_> {
    fn note(
        &mut self,
        name: Option<String>,
        annotation: Option<&TSTypeAnnotation<'_>>,
        params: Option<&oxc_ast::ast::FormalParameters<'_>>,
    ) {
        let Some(name) = name else { return };
        let guard = match (annotation, params) {
            (Some(annotation), Some(params)) => self.guard_of(annotation, params),
            _ => None,
        };
        match self.found.get(&name) {
            None => {
                self.found.insert(name, guard);
            }
            Some(existing) if *existing == guard => {}
            Some(_) => {
                self.found.insert(name, None);
            }
        }
    }

    fn guard_of(
        &mut self,
        annotation: &TSTypeAnnotation<'_>,
        params: &oxc_ast::ast::FormalParameters<'_>,
    ) -> Option<MethodGuard> {
        let TSType::TSTypePredicate(predicate) = &annotation.type_annotation else {
            return None;
        };
        let target = match &predicate.parameter_name {
            TSTypePredicateName::This(_) => GuardTarget::This,
            TSTypePredicateName::Identifier(name) => {
                GuardTarget::Param(params.items.iter().position(|param| {
                    matches!(&param.pattern,
                        oxc_ast::ast::BindingPattern::BindingIdentifier(id) if id.name == name.name)
                })?)
            }
        };
        let type_id = match &predicate.type_annotation {
            Some(annotation) => Some(crate::type_annotation::resolve_type_annotation(
                annotation,
                &mut self.ctx.namespace,
                &mut self.ctx.arena,
            )?),
            None => None,
        };
        Some(MethodGuard {
            target,
            type_id,
            asserts: predicate.asserts,
        })
    }
}

fn key_name(key: &oxc_ast::ast::PropertyKey<'_>) -> Option<String> {
    match key {
        oxc_ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
        _ => None,
    }
}

impl<'a> Visit<'a> for Scan<'_, '_, '_> {
    fn visit_method_definition(&mut self, method: &MethodDefinition<'a>) {
        self.note(
            key_name(&method.key),
            method.value.return_type.as_deref(),
            Some(&method.value.params),
        );
        walk::walk_method_definition(self, method);
    }
    fn visit_ts_method_signature(&mut self, signature: &TSMethodSignature<'a>) {
        self.note(
            key_name(&signature.key),
            signature.return_type.as_deref(),
            Some(&signature.params),
        );
        walk::walk_ts_method_signature(self, signature);
    }
    fn visit_property_definition(&mut self, property: &PropertyDefinition<'a>) {
        let name = key_name(&property.key);
        match &property.value {
            Some(Ex::ArrowFunctionExpression(arrow)) => {
                self.note(name, arrow.return_type.as_deref(), Some(&arrow.params))
            }
            Some(Ex::FunctionExpression(function)) => self.note(
                name,
                function.return_type.as_deref(),
                Some(&function.params),
            ),
            _ => self.note(name, None, None),
        }
        walk::walk_property_definition(self, property);
    }
    fn visit_ts_property_signature(&mut self, property: &TSPropertySignature<'a>) {
        let name = key_name(&property.key);
        match property
            .type_annotation
            .as_ref()
            .map(|a| &a.type_annotation)
        {
            Some(TSType::TSFunctionType(function)) => {
                self.note(name, Some(&function.return_type), Some(&function.params))
            }
            _ => self.note(name, None, None),
        }
        walk::walk_ts_property_signature(self, property);
    }
}

/// Records the method guards of a file, before its bodies are checked.
pub fn collect_method_guards<'ast>(
    program: &'ast oxc_ast::ast::Program<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let mut scan = Scan {
        ctx,
        found: Default::default(),
    };
    scan.visit_program(program);
    let found = scan.found;
    ctx.method_guards = found
        .into_iter()
        .filter_map(|(name, guard)| guard.map(|guard| (name, guard)))
        .collect();
}

/// `a.isLeader()` / `this.assert(x)`: what the call says about the receiver or an argument.
pub(super) fn method_guard_pair(
    call: &oxc_ast::ast::CallExpression,
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
    allow_asserts: bool,
) -> Option<(NarrowState, NarrowState)> {
    let Expression::StaticMemberExpression(member) = &call.callee else {
        return None;
    };
    if member.optional || ctx.method_guards.is_empty() {
        return None;
    }
    let guard = *ctx.method_guards.get(member.property.name.as_str())?;
    if guard.asserts != allow_asserts {
        return None;
    }
    match guard.target {
        GuardTarget::This => {
            let claimed = guard.type_id?;
            let Expression::Identifier(object) = &member.object else {
                return None;
            };
            let symbol = resolve_symbol_id(object, scoping)?;
            Some(by_symbol(ctx, symbol, move |arena, current, want_true| {
                narrow_to_claimed(arena, current, claimed, want_true)
            }))
        }
        GuardTarget::Param(index) => {
            let argument = call.arguments.get(index)?.as_expression()?;
            let Some(claimed) = guard.type_id else {
                // `asserts x`: the argument is truthy afterwards.
                return Some(narrow_condition(argument, scoping, ctx));
            };
            let Expression::Identifier(ident) = argument else {
                return None;
            };
            let symbol = resolve_symbol_id(ident, scoping)?;
            Some(by_symbol(ctx, symbol, move |arena, current, want_true| {
                narrow_to_claimed(arena, current, claimed, want_true)
            }))
        }
    }
}
