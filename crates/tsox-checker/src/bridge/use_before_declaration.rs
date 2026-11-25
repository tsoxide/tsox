// TS2449: a class binding read before its declaration, where the read is evaluated at
// that point and not deferred. A plain value position before the class statement is the
// clear case (`var x = new A(); class A {}`), and so is a heritage base (`class D extends
// B {}` with `B` later) and the operand of `typeof` or `instanceof`. A read inside a
// function, method, accessor or arrow body is deferred — the function may run after the
// class exists — unless the function expression is immediately invoked. Three positions
// inside the class's own span are still evaluated during the declaration and are errors
// even though the read sits after the class starts: a computed property name of one of
// its own members, a static field initializer naming a later class, and a standard
// (non-legacy) decorator on the class or one of its members naming the class itself.
// Type positions never read the value, so a type annotation or a type argument naming a
// later class is not an error, and neither is an ambient (`declare`) class.
//
// Two walks: the first collects every class declaration by symbol id (its span is what
// "before" is measured against), the second walks the program with the position rules.

use oxc_ast::ast::{Class, ClassType, Program};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SymbolId};
use oxc_span::{GetSpan, Span};

use super::context::CheckContext;
use crate::fxhash::FxHashMap;

pub fn report_class_use_before_declaration<'ast>(
    program: &'ast Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let legacy = ctx.option(crate::check_options::OptionField::ExperimentalDecorators);
    let mut collect = Collect {
        classes: FxHashMap::default(),
    };
    collect.visit_program(program);
    let mut pass = Pass {
        ctx,
        scoping,
        classes: &collect.classes,
        legacy,
        functions: Vec::new(),
        class_stack: Vec::new(),
        decorator_of: None,
        computed_name_of: None,
        in_export_binding: false,
        in_instance_initializer: false,
        next_function_invoked: false,
    };
    pass.visit_program(program);
}

struct ClassDecl {
    span: Span,
    declare: bool,
}

struct Collect {
    classes: FxHashMap<SymbolId, ClassDecl>,
}

impl<'ast> Visit<'ast> for Collect {
    fn visit_class(&mut self, it: &Class<'ast>) {
        if it.r#type == ClassType::ClassDeclaration && !it.declare {
            if let Some(id) = &it.id {
                if let Some(symbol_id) = id.symbol_id.get() {
                    self.classes.insert(
                        symbol_id,
                        ClassDecl {
                            span: it.span(),
                            declare: it.declare,
                        },
                    );
                }
            }
        }
        walk::walk_class(self, it);
    }
}

struct FnCtx {
    // A function expression that is the callee of the call it sits in: its body runs
    // now, so a class read inside it is not deferred by it.
    invoked: bool,
}

struct Pass<'a, 'ast, 'src> {
    ctx: &'a mut CheckContext<'ast, 'src>,
    scoping: &'a Scoping,
    classes: &'a FxHashMap<SymbolId, ClassDecl>,
    legacy: bool,
    // The function-likes being walked, innermost last.
    functions: Vec<FnCtx>,
    // The classes whose span we are inside, innermost last. Empty at the top level.
    class_stack: Vec<Option<SymbolId>>,
    // The class a walked decorator belongs to, if it belongs to one.
    decorator_of: Option<SymbolId>,
    // The class a walked computed member key belongs to, if it belongs to one.
    computed_name_of: Option<SymbolId>,
    // True while walking an export clause or an export-equals expression: `export { Y as
    // Z }` and `export = Y` name a class as a module binding, and tsc does not treat
    // that as a runtime read, so a later class is no error there.
    in_export_binding: bool,
    // True while walking the initializer of an instance field or accessor: it runs when
    // an object is built, not with the class declaration, so a class read in it is
    // deferred. A static initializer runs with the declaration and is not deferred.
    in_instance_initializer: bool,
    // Set by `visit_call_expression` when its callee is a function expression, and read
    // (and cleared) by the `visit_function` that follows for it.
    next_function_invoked: bool,
}

impl Pass<'_, '_, '_> {
    // Deferred by a function context: the read is inside a body that may run later (or
    // inside an immediately invoked one that runs now, which still counts). Decorators
    // and computed names use only this, because they run with the declaration no matter
    // what field they sit on.
    fn deferred_by_function(&self) -> bool {
        self.functions.iter().any(|f| !f.invoked)
    }

    // The position rule additionally treats an instance field initializer as deferred.
    fn deferred(&self) -> bool {
        self.deferred_by_function() || self.in_instance_initializer
    }

    fn report(&mut self, name: &str, span: Span) {
        self.ctx.error(
            crate::diagnostic_messages::messages::class_used_before_declaration(name),
            span,
        );
    }
}

impl<'ast> Visit<'ast> for Pass<'_, 'ast, '_> {
    fn visit_class(&mut self, it: &Class<'ast>) {
        // Only the named binding of a declaration is what a reference resolves to; a
        // class expression's own name is internal to it. Every class pushes an entry, with
        // `None` for an expression (or a `declare` class), so a computed member name or a
        // decorator inside a nested class expression is not blamed on the enclosing
        // declaration (`static A = class { [D.D]() {} }` inside `class D`).
        let owner = if it.r#type == ClassType::ClassDeclaration && !it.declare {
            it.id.as_ref().and_then(|id| id.symbol_id.get())
        } else {
            None
        };
        self.class_stack.push(owner);
        walk::walk_class(self, it);
        self.class_stack.pop();
    }

    fn visit_decorator(&mut self, it: &oxc_ast::ast::Decorator<'ast>) {
        // A decorator belongs to the innermost class being walked (its own or one of its
        // members); under legacy decorators tsc does not apply this rule at all.
        let outer = self.decorator_of.take();
        if !self.legacy {
            self.decorator_of = self.class_stack.last().copied().flatten();
        }
        walk::walk_decorator(self, it);
        self.decorator_of = outer;
    }

    fn visit_method_definition(&mut self, it: &oxc_ast::ast::MethodDefinition<'ast>) {
        let outer = self.computed_name_of.take();
        if it.computed {
            self.computed_name_of = self.class_stack.last().copied().flatten();
        }
        walk::walk_method_definition(self, it);
        self.computed_name_of = outer;
    }

    fn visit_property_definition(&mut self, it: &oxc_ast::ast::PropertyDefinition<'ast>) {
        let outer = self.computed_name_of.take();
        if it.computed {
            self.computed_name_of = self.class_stack.last().copied().flatten();
        }
        // An instance field's initializer runs per object, so a class read in it is
        // deferred; decorators and the key still run with the declaration, and the
        // rules for those do not read this flag.
        let outer_init = self.in_instance_initializer;
        self.in_instance_initializer = !it.r#static;
        walk::walk_property_definition(self, it);
        self.in_instance_initializer = outer_init;
        self.computed_name_of = outer;
    }

    fn visit_accessor_property(&mut self, it: &oxc_ast::ast::AccessorProperty<'ast>) {
        let outer = self.computed_name_of.take();
        if it.computed {
            self.computed_name_of = self.class_stack.last().copied().flatten();
        }
        let outer_init = self.in_instance_initializer;
        self.in_instance_initializer = !it.r#static;
        walk::walk_accessor_property(self, it);
        self.in_instance_initializer = outer_init;
        self.computed_name_of = outer;
    }

    fn visit_function(
        &mut self,
        it: &oxc_ast::ast::Function<'ast>,
        flags: oxc_semantic::ScopeFlags,
    ) {
        let invoked = std::mem::take(&mut self.next_function_invoked);
        self.functions.push(FnCtx { invoked });
        walk::walk_function(self, it, flags);
        self.functions.pop();
    }

    fn visit_arrow_function_expression(
        &mut self,
        it: &oxc_ast::ast::ArrowFunctionExpression<'ast>,
    ) {
        let invoked = std::mem::take(&mut self.next_function_invoked);
        self.functions.push(FnCtx { invoked });
        walk::walk_arrow_function_expression(self, it);
        self.functions.pop();
    }

    fn visit_ts_export_assignment(&mut self, it: &oxc_ast::ast::TSExportAssignment<'ast>) {
        let outer = self.in_export_binding;
        self.in_export_binding = true;
        walk::walk_ts_export_assignment(self, it);
        self.in_export_binding = outer;
    }

    fn visit_export_specifier(&mut self, it: &oxc_ast::ast::ExportSpecifier<'ast>) {
        let outer = self.in_export_binding;
        self.in_export_binding = true;
        walk::walk_export_specifier(self, it);
        self.in_export_binding = outer;
    }

    fn visit_call_expression(&mut self, it: &oxc_ast::ast::CallExpression<'ast>) {
        // `(() => A)()` and `(function () { A })()`: the callee is a function expression
        // the call runs at once, so its body is not a deferred context. Mark it for the
        // function visit that is about to happen (the callee is walked first).
        use oxc_ast::ast::Expression;
        if matches!(
            &it.callee,
            Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_)
        ) {
            self.next_function_invoked = true;
        }
        walk::walk_call_expression(self, it);
        // Not consumed (the callee was parenthesized or otherwise not a function): clear
        // it so it never leaks onto an unrelated later function.
        self.next_function_invoked = false;
    }

    fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'ast>) {
        // oxc names a type position the same way (`var x: C`, `T extends C`, `typeof C`
        // in a type): an IdentifierReference that resolves to the same symbol. Only a
        // runtime read or write is what TS2449 is about, and the reference flags carry
        // that: `Type` is a type definition and `ValueAsType` a value named by a type,
        // neither of which reads the class at run time.
        let value_read = it.reference_id.get().is_some_and(|reference_id| {
            let flags = self.scoping.get_reference(reference_id).flags();
            flags.intersects(
                oxc_semantic::ReferenceFlags::Read | oxc_semantic::ReferenceFlags::Write,
            )
        });
        if value_read && !self.in_export_binding {
            if let Some(symbol_id) = super::narrow::resolve_symbol_id(it, self.scoping) {
                if let Some(decl) = self.classes.get(&symbol_id) {
                    // `declare class` never errors (nothing about it is evaluated).
                    if !decl.declare {
                        // Inside a computed member name of the class itself, or a decorator
                        // on the class or one of its members naming the class, the read
                        // happens while the class is being declared.
                        let in_own_computed_name = self.computed_name_of == Some(symbol_id);
                        let in_own_decorator = !self.legacy && self.decorator_of == Some(symbol_id);
                        if (in_own_computed_name && !self.deferred_by_function())
                            || (in_own_decorator && !self.deferred_by_function())
                            || (it.span.start < decl.span.start && !self.deferred())
                        {
                            self.report(&it.name, it.span);
                        }
                    }
                }
            }
        }
        walk::walk_identifier_reference(self, it);
    }
}
