// Where a decorator may stand (tsc's `nodeCanBeDecorated`). A decorator the grammar lets through
// but the language does not is TS1206 at the declaration's first token, the `@`.
//
// The ECMAScript rules are the default: a class, a class expression, a method, accessor or field
// with a body or an initializer position, and nothing else. `experimentalDecorators` adds the
// parameters of a constructor, method or setter that has a body, and takes class expressions away.
// A method with no body (an overload, or an abstract method) is TS1249; an accessor with none, a
// field marked `declare` or `abstract`, and a constructor are TS1206.

use oxc_ast::ast::{
    AccessorProperty, AccessorPropertyType, Class, ClassType, Decorator, FormalParameter,
    MethodDefinition, MethodDefinitionKind, Program, PropertyDefinition, PropertyDefinitionType,
};
use oxc_ast_visit::{Visit, walk};
use oxc_span::Span;

use super::context::CheckContext;
use crate::diagnostic_codes::DiagnosticCode as Code;
use crate::diagnostic_messages::messages;

struct Pass<'a, 'ast, 'src> {
    ctx: &'a mut CheckContext<'ast, 'src>,
    legacy: bool,
    // Whether the parameters of the member being walked may carry decorators.
    parameters_allowed: bool,
}

pub(super) fn check<'ast>(program: &'ast Program<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    let legacy = ctx.option(crate::check_options::OptionField::ExperimentalDecorators);
    let mut pass = Pass {
        ctx,
        legacy,
        parameters_allowed: false,
    };
    pass.visit_program(program);
}

impl Pass<'_, '_, '_> {
    fn not_valid(&mut self, decorators: &[Decorator<'_>]) {
        if let Some(first) = decorators.first() {
            self.ctx.error(
                messages::syntax(
                    Code::DecoratorNotValidHere,
                    "Decorators are not valid here.".to_string(),
                ),
                Span::new(first.span.start, first.span.start + 1),
            );
        }
    }

    fn overload(&mut self, decorators: &[Decorator<'_>]) {
        if let Some(first) = decorators.first() {
            self.ctx.error(
                messages::syntax(
                    Code::DecoratorOnOverload,
                    "A decorator can only decorate a method implementation, not an overload."
                        .to_string(),
                ),
                Span::new(first.span.start, first.span.start + 1),
            );
        }
    }
}

impl<'ast> Visit<'ast> for Pass<'_, 'ast, '_> {
    fn visit_class(&mut self, it: &Class<'ast>) {
        if self.legacy && it.r#type == ClassType::ClassExpression {
            self.not_valid(&it.decorators);
        }
        walk::walk_class(self, it);
    }

    fn visit_method_definition(&mut self, it: &MethodDefinition<'ast>) {
        let has_body = it.value.body.is_some();
        match it.kind {
            MethodDefinitionKind::Constructor => self.not_valid(&it.decorators),
            MethodDefinitionKind::Method if !has_body => self.overload(&it.decorators),
            MethodDefinitionKind::Get | MethodDefinitionKind::Set if !has_body => {
                self.not_valid(&it.decorators)
            }
            _ => {}
        }
        let outer = self.parameters_allowed;
        self.parameters_allowed = self.legacy
            && has_body
            && matches!(
                it.kind,
                MethodDefinitionKind::Constructor
                    | MethodDefinitionKind::Method
                    | MethodDefinitionKind::Set
            );
        walk::walk_method_definition(self, it);
        self.parameters_allowed = outer;
    }

    fn visit_property_definition(&mut self, it: &PropertyDefinition<'ast>) {
        if it.declare || it.r#type == PropertyDefinitionType::TSAbstractPropertyDefinition {
            self.not_valid(&it.decorators);
        }
        walk::walk_property_definition(self, it);
    }

    fn visit_accessor_property(&mut self, it: &AccessorProperty<'ast>) {
        if matches!(it.r#type, AccessorPropertyType::TSAbstractAccessorProperty) {
            self.not_valid(&it.decorators);
        }
        walk::walk_accessor_property(self, it);
    }

    fn visit_formal_parameter(&mut self, it: &FormalParameter<'ast>) {
        if !self.parameters_allowed {
            self.not_valid(&it.decorators);
        }
        walk::walk_formal_parameter(self, it);
    }
}
