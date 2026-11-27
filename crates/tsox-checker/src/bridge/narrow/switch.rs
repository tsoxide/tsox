// Owns: narrowing by `switch` cases and the default clause, and the discriminant of a switch.

use super::*;

// The discriminant shapes a `switch` can narrow on: `kind`, `shape.kind` (the
// discriminated-union form) and `typeof x`. The narrowing overlay is keyed by a
// single symbol, so a discriminant that does not resolve to one (a call, an index,
// a longer chain) has nowhere to record a result and narrows nothing.
pub enum SwitchDiscriminant {
    Identifier(SymbolId),
    Property(SymbolId, String),
    Typeof(SymbolId),
    // `switch (true)`: each clause's test is a condition, narrowing as `if` would.
    True,
}

pub fn switch_discriminant(expr: &Expression, scoping: &Scoping) -> Option<SwitchDiscriminant> {
    match expr {
        Expression::ParenthesizedExpression(inner) => {
            switch_discriminant(&inner.expression, scoping)
        }
        Expression::BooleanLiteral(literal) if literal.value => Some(SwitchDiscriminant::True),
        Expression::Identifier(ident) => {
            resolve_symbol_id(ident, scoping).map(SwitchDiscriminant::Identifier)
        }
        Expression::StaticMemberExpression(member) if !member.optional => {
            let Expression::Identifier(ident) = &member.object else {
                return None;
            };
            let symbol_id = resolve_symbol_id(ident, scoping)?;
            Some(SwitchDiscriminant::Property(
                symbol_id,
                member.property.name.to_string(),
            ))
        }
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::Typeof => {
            let Expression::Identifier(ident) = &unary.argument else {
                return None;
            };
            resolve_symbol_id(ident, scoping).map(SwitchDiscriminant::Typeof)
        }
        _ => None,
    }
}

pub(super) fn current_type(ctx: &CheckContext<'_, '_>, symbol_id: SymbolId) -> Option<TypeId> {
    ctx.narrow
        .get(symbol_id)
        .or_else(|| ctx.symbols.get(symbol_id))
}

// The narrowing for a case body reached through any of `tests` (several when empty
// case labels are grouped: `case "a": case "b": body`). If any one test cannot be
// resolved to a literal (or a known typeof tag) the body could be reached with a
// value this checker cannot describe, so nothing is narrowed.
pub fn narrow_switch_case(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    if matches!(discriminant, SwitchDiscriminant::True) {
        // A body reached through one clause test holds what the test establishes. Several
        // grouped tests would need the union of their states, which is left unnarrowed.
        return match tests {
            [test] => narrow_condition(test, scoping, ctx).0,
            _ => NarrowState::new(),
        };
    }
    let state = narrow_switch_case_inner(discriminant, tests, ctx);
    propagate_destructured(ctx, state)
}

// `const { kind } = obj; switch (kind) { case "a": ... }`: the cases narrow `obj` as
// `switch (obj.kind)` would.
fn alias_root_state(
    ctx: &mut CheckContext<'_, '_>,
    alias: SymbolId,
    literals: &[TypeId],
    positive: bool,
) -> NarrowState {
    let Some((root, property)) = ctx.discriminant_aliases.get(&alias).cloned() else {
        return NarrowState::new();
    };
    let Some(current) = current_type(ctx, root) else {
        return NarrowState::new();
    };
    let narrowed =
        narrow_by_property_literals(&mut ctx.arena, current, &property, literals, positive);
    NarrowState::from_single(root, narrowed)
}

pub(super) fn narrow_switch_case_inner(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    match discriminant {
        SwitchDiscriminant::True => NarrowState::new(),
        SwitchDiscriminant::Identifier(symbol_id) => {
            let Some(literals) = resolve_all_literals(tests, &mut ctx.arena) else {
                return NarrowState::new();
            };
            let alias = alias_root_state(ctx, *symbol_id, &literals, true);
            let Some(current) = current_type(ctx, *symbol_id) else {
                return alias;
            };
            let narrowed = narrow_by_literals(&mut ctx.arena, current, &literals, true);
            let mut state = NarrowState::from_single(*symbol_id, narrowed);
            state.extend(alias);
            state
        }
        SwitchDiscriminant::Property(symbol_id, property) => {
            let Some(literals) = resolve_all_literals(tests, &mut ctx.arena) else {
                return NarrowState::new();
            };
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let narrowed =
                narrow_by_property_literals(&mut ctx.arena, current, property, &literals, true);
            NarrowState::from_single(*symbol_id, narrowed)
        }
        SwitchDiscriminant::Typeof(symbol_id) => {
            let Some(tags) = resolve_all_typeof_tags(tests) else {
                return NarrowState::new();
            };
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let mut parts = Vec::with_capacity(tags.len());
            for tag in &tags {
                parts.push(narrow_by_typeof(&mut ctx.arena, current, tag, true));
            }
            let narrowed = ctx.arena.alloc_union(parts);
            NarrowState::from_single(*symbol_id, narrowed)
        }
    }
}

// A switch's default: whatever remains once every sibling case's test is ruled
// out. A test this checker cannot resolve is simply left out of the exclusion,
// which keeps more members than strictly necessary and is always safe.
pub fn narrow_switch_default(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    scoping: &Scoping,
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    if matches!(discriminant, SwitchDiscriminant::True) {
        // Every clause test was false, one after the other.
        let saved = ctx.narrow.clone();
        let mut gathered = NarrowState::new();
        for test in tests {
            let (_, when_false) = narrow_condition(test, scoping, ctx);
            ctx.narrow.extend(when_false.clone());
            gathered.extend(when_false);
        }
        ctx.narrow = saved;
        return gathered;
    }
    let state = narrow_switch_default_inner(discriminant, tests, ctx);
    propagate_destructured(ctx, state)
}

pub(super) fn narrow_switch_default_inner(
    discriminant: &SwitchDiscriminant,
    tests: &[&Expression],
    ctx: &mut CheckContext<'_, '_>,
) -> NarrowState {
    match discriminant {
        SwitchDiscriminant::True => NarrowState::new(),
        SwitchDiscriminant::Identifier(symbol_id) => {
            let literals: Vec<TypeId> = tests
                .iter()
                .filter_map(|test| resolve_literal(test, &mut ctx.arena))
                .collect();
            let alias = alias_root_state(ctx, *symbol_id, &literals, false);
            let Some(current) = current_type(ctx, *symbol_id) else {
                return alias;
            };
            let narrowed = narrow_by_literals(&mut ctx.arena, current, &literals, false);
            let mut state = NarrowState::from_single(*symbol_id, narrowed);
            state.extend(alias);
            state
        }
        SwitchDiscriminant::Property(symbol_id, property) => {
            let literals: Vec<TypeId> = tests
                .iter()
                .filter_map(|test| resolve_literal(test, &mut ctx.arena))
                .collect();
            let Some(current) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            let narrowed =
                narrow_by_property_literals(&mut ctx.arena, current, property, &literals, false);
            NarrowState::from_single(*symbol_id, narrowed)
        }
        SwitchDiscriminant::Typeof(symbol_id) => {
            let Some(mut narrowed) = current_type(ctx, *symbol_id) else {
                return NarrowState::new();
            };
            for test in tests {
                if let Expression::StringLiteral(tag) = test
                    && is_known_typeof_tag(&tag.value)
                {
                    narrowed = narrow_by_typeof(&mut ctx.arena, narrowed, &tag.value, false);
                }
            }
            NarrowState::from_single(*symbol_id, narrowed)
        }
    }
}

// Problem: a `switch` whose cases cover every value of its discriminant was treated as if a value
// could still miss all of them, so a function that returns from every case was reported as
// lacking an ending return (TS2366), a variable assigned in every case as possibly unassigned
// (TS2454), and the code after such a switch as reachable.
// Picked: tsc's `isExhaustiveSwitchStatement`. For `switch (typeof x)`, the operand's type (a
// constrained type parameter by its constraint) with every case's tag ruled out must be never.
// Otherwise the discriminant's type must be a union of unit types (literals, `null`,
// `undefined`, `boolean` as `true | false`, enum members), every case's type must be a unit type
// as well, and each member of the discriminant must be one of the cases.
// Cost: a discriminant that is not a plain variable (`typeof a.b`) is not judged exhaustive, and
// a case whose type the checker does not know leaves the switch open, which is the safe side:
// it can only add a missing-return report that tsc would not make, as before.
pub fn switch_is_exhaustive(
    discriminant: Option<&SwitchDiscriminant>,
    discriminant_type: TypeId,
    case_types: &[TypeId],
    tests: &[&Expression],
    ctx: &mut CheckContext<'_, '_>,
) -> bool {
    if tests.is_empty() {
        return false;
    }
    if let Some(SwitchDiscriminant::Typeof(symbol_id)) = discriminant {
        let Some(tags) = resolve_all_typeof_tags(tests) else {
            return false;
        };
        let Some(mut remaining) = current_type(ctx, *symbol_id) else {
            return false;
        };
        if let Type::GenericParameter(_, _, Some(constraint)) = *ctx.arena.get(remaining) {
            remaining = constraint;
        }
        for tag in &tags {
            remaining = narrow_by_typeof(&mut ctx.arena, remaining, tag, false);
        }
        return matches!(ctx.arena.get(remaining), Type::Never);
    }
    let Some(wanted) = unit_values(&ctx.arena, discriminant_type, true) else {
        return false;
    };
    let mut covered: Vec<UnitValue> = Vec::new();
    for &case_type in case_types {
        let Some(values) = unit_values(&ctx.arena, case_type, false) else {
            return false;
        };
        covered.extend(values);
    }
    !wanted.is_empty() && wanted.iter().all(|value| covered.contains(value))
}

#[derive(Clone, PartialEq)]
enum UnitValue {
    Text(String),
    Number(u64),
    Boolean(bool),
    Null,
    Undefined,
}

// The unit values a type can hold, or None when it holds anything wider (`string`, an object, a
// type parameter) or is not known. `never` holds none; `boolean` is `true` and `false` when
// `expand_boolean` is set.
fn unit_values(arena: &TypeArena, id: TypeId, expand_boolean: bool) -> Option<Vec<UnitValue>> {
    let members = union_members(arena, id).unwrap_or_else(|| vec![id]);
    let mut values = Vec::with_capacity(members.len());
    for member in members {
        match arena.get(member) {
            Type::StringLiteral(text) => values.push(UnitValue::Text(text.clone())),
            Type::NumberLiteral(number) => values.push(UnitValue::Number(number.to_bits())),
            Type::BooleanLiteral(value) => values.push(UnitValue::Boolean(*value)),
            // The discriminant may be `boolean` (a union of two units); a case may not: a case of
            // type `boolean` is not one value, so it covers nothing (`switch (true)` with a
            // comparison for a label).
            Type::Boolean if expand_boolean => {
                values.push(UnitValue::Boolean(true));
                values.push(UnitValue::Boolean(false));
            }
            Type::Null => values.push(UnitValue::Null),
            Type::Undefined => values.push(UnitValue::Undefined),
            Type::Never => {}
            _ => return None,
        }
    }
    Some(values)
}

// `switch (typeof x)` with a tag written in an earlier case: tsc takes the repeat for a case no
// value reaches (its witness list holds `undefined` there), so the body is narrowed to `never`.
// `tests` are the labels the body is reached through and `earlier` every label of the cases
// before them; what is returned is `tests` without a tag `earlier` or an earlier label of
// `tests` already named. Any other discriminant is returned as it is.
pub fn without_repeated_tags<'a, 'ast>(
    discriminant: &SwitchDiscriminant,
    tests: &[&'a Expression<'ast>],
    earlier: &[&Expression<'_>],
) -> Vec<&'a Expression<'ast>> {
    if !matches!(discriminant, SwitchDiscriminant::Typeof(_)) {
        return tests.to_vec();
    }
    let tag_of = |test: &Expression| match test {
        Expression::StringLiteral(tag) => Some(tag.value.to_string()),
        _ => None,
    };
    let mut seen: Vec<String> = earlier.iter().filter_map(|test| tag_of(test)).collect();
    let mut kept = Vec::with_capacity(tests.len());
    for test in tests {
        match tag_of(test) {
            Some(tag) if seen.contains(&tag) => {}
            Some(tag) => {
                seen.push(tag);
                kept.push(*test);
            }
            None => kept.push(*test),
        }
    }
    kept
}
