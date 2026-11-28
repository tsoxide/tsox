use oxc_ast::ast::{Class, Program, Statement};
use oxc_span::GetSpan;

use crate::namespace::Resolution;

use super::context::CheckContext;

// `class C implements I`: the instance type of C must be assignable to I (TS2420). Only a
// class and an interface that both resolve completely are compared, so a shape this checker
// cannot see never produces a report; the cost is that such a class goes unchecked.
pub fn check_class_implements<'ast>(program: &Program<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    for statement in &program.body {
        let class = match statement {
            Statement::ClassDeclaration(class) => Some(class),
            Statement::ExportDeclaration(export) => match &export.declaration {
                oxc_ast::ast::Declaration::ClassDeclaration(class) => Some(class),
                _ => None,
            },
            _ => None,
        };
        if let Some(class) = class
            && let Some(id) = class.id.as_ref()
        {
            let names = class
                .body
                .body
                .iter()
                .filter_map(|element| match element {
                    oxc_ast::ast::ClassElement::PropertyDefinition(p)
                        if p.r#type
                            == oxc_ast::ast::PropertyDefinitionType::TSAbstractPropertyDefinition => {
                        p.key.static_name().map(|n| n.to_string())
                    }
                    oxc_ast::ast::ClassElement::MethodDefinition(m)
                        if m.r#type
                            == oxc_ast::ast::MethodDefinitionType::TSAbstractMethodDefinition =>
                    {
                        m.key.static_name().map(|n| n.to_string())
                    }
                    _ => None,
                })
                .collect();
            ctx.abstract_members.insert(id.name.to_string(), names);
            let mut statics: std::collections::HashSet<String> = class
                .heritage
                .as_ref()
                .and_then(|h| match &h.expression {
                    oxc_ast::ast::Expression::Identifier(base) => {
                        ctx.static_members.get(base.name.as_str())
                    }
                    _ => None,
                })
                .cloned()
                .unwrap_or_default();
            for element in &class.body.body {
                let (key, is_static) = match element {
                    oxc_ast::ast::ClassElement::PropertyDefinition(p) => (&p.key, p.r#static),
                    oxc_ast::ast::ClassElement::MethodDefinition(m) => (&m.key, m.r#static),
                    _ => continue,
                };
                if is_static && let Some(name) = key.static_name() {
                    statics.insert(name.to_string());
                }
            }
            ctx.static_members.insert(id.name.to_string(), statics);
        }
    }
    for statement in &program.body {
        match statement {
            Statement::ClassDeclaration(class) => check_class(class, ctx),
            Statement::TSInterfaceDeclaration(interface) => check_interface(interface, ctx),
            Statement::ExportDeclaration(export) => match &export.declaration {
                oxc_ast::ast::Declaration::ClassDeclaration(class) => check_class(class, ctx),
                oxc_ast::ast::Declaration::TSInterfaceDeclaration(interface) => {
                    check_interface(interface, ctx)
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn check_class<'ast>(class: &Class<'ast>, ctx: &mut CheckContext<'ast, '_>) {
    let Some(id) = class.id.as_ref() else {
        return;
    };
    if class.implements.is_empty() && class.heritage.is_none() {
        return;
    }
    let Resolution::Resolved(instance) = ctx.namespace.resolve(&id.name, &mut ctx.arena) else {
        return;
    };
    check_extends(class, id, instance, ctx);
    for implemented in &class.implements {
        let Some(name) = crate::type_annotation::dotted_type_name(&implemented.expression) else {
            continue;
        };
        let Some(interface) = crate::type_annotation::resolve_named_reference(
            &name,
            implemented.type_arguments.as_deref(),
            implemented.span(),
            &mut ctx.namespace,
            &mut ctx.arena,
        ) else {
            continue;
        };
        if crate::subtyping::is_subtype(&ctx.arena, instance, interface) {
            continue;
        }
        // Every property both sides have with disagreeing types is reported at the class
        // member (TS2416), as tsc does; with none of those, a missing member is reported once
        // at the class (TS2420).
        let conflicts = conflicting_properties(ctx, instance, interface);
        if conflicts.is_empty() {
            ctx.error(
                crate::diagnostic_messages::messages::class_incorrectly_implements(
                    &ctx.arena, &id.name, instance, interface,
                ),
                implemented.span(),
            );
        }
        for property in conflicts {
            let span = member_span(class, &property).unwrap_or_else(|| implemented.span());
            let (own, inherited) = property_types(ctx, instance, interface, &property);
            ctx.error(
                crate::diagnostic_messages::messages::class_property_incompatible(
                    &ctx.arena, &property, &id.name, own, inherited, interface,
                ),
                span,
            );
        }
    }
}

fn conflicting_properties(
    ctx: &CheckContext<'_, '_>,
    instance: crate::arena::TypeId,
    interface: crate::arena::TypeId,
) -> Vec<String> {
    use crate::types::Type;
    let (Type::Object(have), Type::Object(want)) =
        (ctx.arena.get(instance), ctx.arena.get(interface))
    else {
        return Vec::new();
    };
    want.properties
        .iter()
        .filter(|property| {
            have.properties.iter().any(|own| {
                own.name == property.name
                    && !crate::subtyping::is_subtype(&ctx.arena, own.type_id, property.type_id)
            })
        })
        .map(|property| property.name.to_string())
        .collect()
}

fn first_incompatible_property(
    ctx: &CheckContext<'_, '_>,
    instance: crate::arena::TypeId,
    interface: crate::arena::TypeId,
) -> Option<(String, bool)> {
    use crate::types::Type;
    let (Type::Object(have), Type::Object(want)) =
        (ctx.arena.get(instance), ctx.arena.get(interface))
    else {
        return None;
    };
    for property in want.properties.iter() {
        match have.properties.iter().find(|p| p.name == property.name) {
            None if !property.optional => return Some((property.name.to_string(), false)),
            None => {}
            Some(own) => {
                if !crate::subtyping::is_subtype(&ctx.arena, own.type_id, property.type_id)
                    && !overloaded_methods_compatible(ctx, own, property)
                {
                    return Some((property.name.to_string(), true));
                }
            }
        }
    }
    None
}

// tsc relates methods (unlike function-typed properties) with bivariant parameters, so a
// derived overload set whose signatures are narrower than the base method's (`(x: 'a')` and
// `(x: 'b')` against `(x: string)`) still extends it. When either side is an overload set of
// methods, the pair is taken as compatible if the relation holds in either direction.
fn overloaded_methods_compatible(
    ctx: &CheckContext<'_, '_>,
    own: &crate::types::PropertyEntry,
    want: &crate::types::PropertyEntry,
) -> bool {
    use crate::types::Type;
    let is_overload_set = |id| matches!(ctx.arena.get(id), Type::Intersection(_));
    own.is_method
        && want.is_method
        && (is_overload_set(own.type_id) || is_overload_set(want.type_id))
        && crate::subtyping::is_subtype(&ctx.arena, want.type_id, own.type_id)
}

// The types a derived and a base type give property `name`.
fn property_types(
    ctx: &CheckContext<'_, '_>,
    derived: crate::arena::TypeId,
    base: crate::arena::TypeId,
    name: &str,
) -> (crate::arena::TypeId, crate::arena::TypeId) {
    use crate::types::Type;
    let find = |ty: crate::arena::TypeId| match ctx.arena.get(ty) {
        Type::Object(object) => object
            .properties
            .iter()
            .find(|p| &*p.name == name)
            .map(|p| p.type_id),
        _ => None,
    };
    (find(derived).unwrap_or(derived), find(base).unwrap_or(base))
}

fn member_span(class: &Class<'_>, name: &str) -> Option<oxc_span::Span> {
    use oxc_ast::ast::ClassElement;
    class.body.body.iter().find_map(|element| match element {
        ClassElement::PropertyDefinition(p) if p.key.static_name().as_deref() == Some(name) => {
            Some(p.key.span())
        }
        ClassElement::MethodDefinition(m) if m.key.static_name().as_deref() == Some(name) => {
            Some(m.key.span())
        }
        _ => None,
    })
}

// A `#name` member belongs to the class that declares it, so a subclass may declare its own
// `#name` with another type (or none) without conflicting with the base. The relation between
// a class and its base is judged on the remaining members. An instance with no `#` member is
// returned as it is.
fn without_private_names(
    ctx: &mut CheckContext<'_, '_>,
    id: crate::arena::TypeId,
) -> crate::arena::TypeId {
    use crate::types::{ObjectType, Type};
    match ctx.arena.get(id).clone() {
        Type::Object(object) if object.properties.iter().any(|p| p.name.starts_with('#')) => {
            let kept = object
                .properties
                .iter()
                .filter(|p| !p.name.starts_with('#'))
                .cloned()
                .collect::<Vec<_>>();
            ctx.arena.alloc(Type::Object(ObjectType::new(kept)))
        }
        Type::Intersection(members) => {
            let stripped = members
                .iter()
                .map(|&m| without_private_names(ctx, m))
                .collect::<Vec<_>>();
            if stripped == members {
                id
            } else {
                ctx.arena.alloc_intersection(stripped).unwrap_or(id)
            }
        }
        _ => id,
    }
}

// `class D extends B`: D's instance must be assignable to B's. A property both declare with
// conflicting types is reported at D's own member (TS2416); any other mismatch at the class
// name (TS2415).
fn check_extends<'ast>(
    class: &Class<'ast>,
    id: &oxc_ast::ast::BindingIdentifier<'ast>,
    instance: crate::arena::TypeId,
    ctx: &mut CheckContext<'ast, '_>,
) {
    let Some(heritage) = &class.heritage else {
        return;
    };
    let oxc_ast::ast::Expression::Identifier(base_name) = &heritage.expression else {
        return;
    };
    let Some(base) = crate::type_annotation::resolve_named_reference(
        &base_name.name,
        heritage.type_arguments.as_deref(),
        base_name.span,
        &mut ctx.namespace,
        &mut ctx.arena,
    ) else {
        return;
    };
    if ctx.option(crate::check_options::OptionField::NoImplicitOverride) {
        report_missing_overrides(class, &base_name.name, base, ctx);
    }
    let (instance, base_for_check) = (
        without_private_names(ctx, instance),
        without_private_names(ctx, base),
    );
    if crate::subtyping::is_subtype(&ctx.arena, instance, base_for_check) {
        return;
    }
    // Only a property both sides declare with conflicting types is reported; a base the
    // namespace models only partly must not turn into an invented mismatch.
    let Some((property, true)) = first_incompatible_property(ctx, instance, base_for_check) else {
        return;
    };
    match member_span(class, &property) {
        Some(span) => {
            let (own, inherited) = property_types(ctx, instance, base, &property);
            ctx.error(
                crate::diagnostic_messages::messages::class_property_incompatible(
                    &ctx.arena, &property, &id.name, own, inherited, base,
                ),
                span,
            )
        }
        None => ctx.error(
            crate::diagnostic_messages::messages::class_incorrectly_extends(
                &ctx.arena, &id.name, instance, base,
            ),
            id.span,
        ),
    }
}

// `interface I extends B`: I as a whole (inherited members overridden by its own) must be
// assignable to B (TS2430). Index signatures are checked against the properties declared in
// the same body (TS2411) and a second index signature of one key type is a duplicate (TS2374).
fn check_interface<'ast>(
    interface: &oxc_ast::ast::TSInterfaceDeclaration<'ast>,
    ctx: &mut CheckContext<'ast, '_>,
) {
    check_index_signatures(&interface.body.body, ctx);
    if interface.extends.is_empty() {
        return;
    }
    let Resolution::Resolved(derived) = ctx.namespace.resolve(&interface.id.name, &mut ctx.arena)
    else {
        return;
    };
    for heritage in &interface.extends {
        let Some(base) = crate::type_annotation::resolve_interface_base(
            heritage,
            &mut ctx.namespace,
            &mut ctx.arena,
        ) else {
            continue;
        };
        // A base whose type arguments failed to resolve (`Collection<error, error>`) is not
        // judged: what a member would be against it is unknown, and that was reported already.
        if ctx.arena.app_parts(base).is_some_and(|(_, arguments)| {
            arguments
                .iter()
                .any(|&argument| matches!(ctx.arena.get(argument), crate::types::Type::Error))
        }) {
            continue;
        }
        // Only a property both sides declare with conflicting types is reported. A base the
        // namespace models only partly (a class, say) can leave the derived shape without
        // members it really has, which must not read as a mismatch.
        if !crate::subtyping::is_subtype(&ctx.arena, derived, base)
            && matches!(
                first_incompatible_property(ctx, derived, base),
                Some((_, true))
            )
        {
            ctx.error(
                crate::diagnostic_messages::messages::interface_incorrectly_extends(
                    &ctx.arena,
                    &interface.id.name,
                    derived,
                    base,
                ),
                interface.id.span,
            );
            continue;
        }
        // tsc also reports TS2430 when the derived interface's string or number index
        // signature is not a subtype of the base's, even when every named property agrees
        // (`interface G extends E, F` where the two bases' index signatures disagree). The
        // whole-object `is_subtype` check above does not fire here because tsox's index
        // signatures are not part of the property walk `first_incompatible_property` runs.
        if let Some(kind) = first_incompatible_index(ctx, derived, base) {
            ctx.error(
                crate::diagnostic_messages::messages::interface_incorrectly_extends(
                    &ctx.arena,
                    &interface.id.name,
                    derived,
                    base,
                ),
                interface.id.span,
            );
            let _ = kind; // reported via the same message as a property mismatch
        }
    }
}

// The first index signature (string or number) the derived interface declares or inherits that
// is not a subtype of the base's corresponding index signature, as tsc's TS2430 check sees it.
// Returns `"string"` or `"number"` so a caller can name the key kind in a future, more
// specific message; today both kinds share the generic `interface_incorrectly_extends` message.
fn first_incompatible_index(
    ctx: &CheckContext<'_, '_>,
    derived: crate::arena::TypeId,
    base: crate::arena::TypeId,
) -> Option<&'static str> {
    use crate::types::Type;
    let (Type::Object(derived_obj), Type::Object(base_obj)) =
        (ctx.arena.get(derived), ctx.arena.get(base))
    else {
        return None;
    };
    for (derived_idx, base_idx, label) in [
        (
            derived_obj.string_index.as_ref(),
            base_obj.string_index.as_ref(),
            "string",
        ),
        (
            derived_obj.number_index.as_ref(),
            base_obj.number_index.as_ref(),
            "number",
        ),
    ] {
        let (Some(derived_idx), Some(base_idx)) = (derived_idx, base_idx) else {
            continue;
        };
        if !crate::subtyping::is_subtype(&ctx.arena, derived_idx.value, base_idx.value) {
            return Some(label);
        }
    }
    None
}

fn check_index_signatures<'ast>(
    members: &[oxc_ast::ast::TSSignature<'ast>],
    ctx: &mut CheckContext<'ast, '_>,
) {
    use oxc_ast::ast::{TSSignature, TSType};
    let mut string_index: Option<crate::arena::TypeId> = None;
    let mut number_index: Option<crate::arena::TypeId> = None;
    let mut seen_string: Option<oxc_span::Span> = None;
    let mut seen_number: Option<oxc_span::Span> = None;
    for member in members {
        let TSSignature::TSIndexSignature(signature) = member else {
            continue;
        };
        let (seen, slot, label) = match &signature.parameter.type_annotation.type_annotation {
            TSType::TSStringKeyword(_) => (&mut seen_string, &mut string_index, "string"),
            TSType::TSNumberKeyword(_) => (&mut seen_number, &mut number_index, "number"),
            _ => continue,
        };
        if let Some(first) = *seen {
            // tsc reports every declaration of the clashing signature, the first too.
            for span in [first, signature.parameter.span] {
                ctx.error(
                    crate::diagnostic_messages::messages::duplicate_index_signature(label),
                    span,
                );
            }
            continue;
        }
        *seen = Some(signature.parameter.span);
        *slot = crate::type_annotation::resolve_type_annotation(
            &signature.type_annotation,
            &mut ctx.namespace,
            &mut ctx.arena,
        );
    }
    for member in members {
        let TSSignature::TSPropertySignature(property) = member else {
            continue;
        };
        let Some(name) = property.key.static_name() else {
            continue;
        };
        let Some(annotation) = &property.type_annotation else {
            continue;
        };
        let Some(own) = crate::type_annotation::resolve_type_annotation(
            annotation,
            &mut ctx.namespace,
            &mut ctx.arena,
        ) else {
            continue;
        };
        let numeric = name.parse::<f64>().is_ok();
        for (index, label) in [
            (string_index, "string"),
            (number_index.filter(|_| numeric), "number"),
        ] {
            let Some(index) = index else { continue };
            if !crate::subtyping::is_subtype(&ctx.arena, own, index) {
                ctx.error(
                    crate::diagnostic_messages::messages::property_not_assignable_to_index(
                        &ctx.arena, &name, own, label, index,
                    ),
                    property.key.span(),
                );
            }
        }
    }
}

// `noImplicitOverride`: an instance member that redeclares one of the base class's needs the
// `override` modifier (TS4114).
fn report_missing_overrides<'ast>(
    class: &Class<'ast>,
    base_name: &str,
    base: crate::arena::TypeId,
    ctx: &mut CheckContext<'ast, '_>,
) {
    use oxc_ast::ast::{ClassElement, MethodDefinitionKind};
    let crate::types::Type::Object(base_object) = ctx.arena.get(base) else {
        return;
    };
    let base_members: Vec<std::rc::Rc<str>> = base_object
        .properties
        .iter()
        .map(|p| p.name.clone())
        .collect();
    // tsc names the base as written with its arguments (`C3<U>`), not the bare class name.
    let base_shown = crate::type_display::display_type(&ctx.arena, base);
    for element in &class.body.body {
        let (key, is_static, has_override, skip) = match element {
            ClassElement::PropertyDefinition(p) => (&p.key, p.r#static, p.r#override, false),
            ClassElement::MethodDefinition(m) => (
                &m.key,
                m.r#static,
                m.r#override,
                m.kind == MethodDefinitionKind::Constructor,
            ),
            _ => continue,
        };
        if has_override || skip {
            continue;
        }
        let Some(name) = key.static_name() else {
            continue;
        };
        if is_static {
            if ctx
                .static_members
                .get(base_name)
                .is_some_and(|set| set.contains(name.as_ref()))
            {
                ctx.error(
                    crate::diagnostic_messages::messages::missing_override_modifier(base_name),
                    key.span(),
                );
            }
            continue;
        }
        let is_abstract = ctx
            .abstract_members
            .get(base_name)
            .is_some_and(|set| set.contains(name.as_ref()));
        if !is_abstract && base_members.iter().any(|m| m.as_ref() == name.as_ref()) {
            ctx.error(
                crate::diagnostic_messages::messages::missing_override_modifier(&base_shown),
                key.span(),
            );
        }
    }
}
