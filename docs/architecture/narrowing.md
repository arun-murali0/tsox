# Control-Flow Narrowing

> **Architecture case study:** introducing flow-sensitive types while keeping declared types stable.

## 1. The problem

A variable can have a broad declared type but a narrower type on a particular control-flow path:

```ts
function f(x: string | null) {
    if (x !== null) {
        // x: string here
    }
}
```

Control-Flow Narrowing introduced a dedicated flow state instead of mutating the permanent symbol type.

Fixtures: `equality_null_narrowing.ts`, `truthy_narrowing.ts`, `typeof_narrowing_string_branch.ts`, `typeof_narrowing_else_branch_mismatch.ts`, `typeof_object_narrows_to_the_object_member.ts`, `typeof_function_narrows_to_the_function_member.ts`, `typeof_object_keeps_null.ts`, `nested_conditions_compose.ts`, `unknown_typeof_tag_narrows_nothing.ts`, `local_annotated_variable_is_registered.ts`, `local_annotated_variable_without_initializer_is_registered.ts`.

## 2. `SymbolTypeMap` versus `NarrowState`

The architecture separates two concepts:

```text
SymbolTypeMap
    = declared/inferred semantic type

NarrowState
    = current flow-sensitive override
```

`NarrowState` stores `(SymbolId, TypeId)` pairs. The use of semantic `SymbolId` means narrowing follows lexical identity rather than source spelling.

## 3. Why Oxc semantic symbols are consumed

Oxc already resolves identifiers to symbols/scopes. tsox consumes that result instead of rebuilding lexical binding analysis.

```text
IdentifierReference
       -> Oxc Scoping
       -> SymbolId
       -> NarrowState
```

This matches the project's dependency rule: reuse expensive front-end infrastructure, own TypeScript type semantics.

## 4. Branch states

`narrow_condition()` returns a pair of states:

```text
condition -> (true_state, false_state)
```

The statement checker saves the old state, applies the branch state, checks the branch, and restores the old state.

This is an **environment overlay** pattern. Flow information is temporary and path-specific.

The pair is computed from the overlay's current type for that symbol, falling back to the declared type only when the symbol has not been narrowed yet. That is what makes narrowing compose: an inner condition refines what the enclosing branch already established instead of restarting from the declared type.

```ts
if (typeof value !== "number") {  // value: string | null
    if (value !== null) {         // value: string, not string | number | null
    }
}
```

## 5. `typeof`, nullish, and truthiness

The narrowing layer contains dedicated logic for `typeof` tags, nullish checks, truthiness/falsiness, and union-member filtering. These operations work on semantic `TypeId` values, not AST syntax after the initial condition has been decoded.

The modelled `typeof` tags are `string`, `number`, `boolean`, `undefined`, `object`, and `function`. `null` answers to the `object` tag, as it does at runtime, so `typeof x === "object"` does not remove `null` from `x`.

A tag outside that set, `"symbol"` or `"bigint"`, would match no member of any union and would narrow the true branch to `never`, turning a gap in the type model into errors reported against the source. Such a condition narrows nothing instead.

## 6. Why not mutate the symbol type

Changing `SymbolTypeMap[x]` inside a branch would incorrectly make the narrow survive after the branch. The separate state layer prevents that semantic leak.

## 7. Conservative recovery

When a condition cannot be understood safely, the implementation prefers to retain the existing type rather than invent a false precise type. For a compiler, a conservative result is often safer than a wrong narrow that hides an error.

## 8. Why this design scales

The same state mechanism handles `!`, assignments, early returns, loops, switch
discrimination, discriminated unions, `&&` / `||`, `in` and `instanceof`. Sections 10
to 15 describe each. No new global type environment was needed for any of them: every
one is an operator-specific handler that returns a pair of overlays, plus, for control
flow, a rule for how overlays are joined.

What the mechanism does not yet cover is listed in section 16.

## 9. What the first Control-Flow Narrowing milestone completed

- symbol-aware flow state
- branch-local narrowing
- `typeof` narrowing
- nullish narrowing
- truthiness narrowing
- state save/restore
- preservation of declared types

The key lesson is: **flow semantics belong in a separate layer over the normal symbol/type environment.**

## 10. Joins: what the code after a branch sees

An overlay that is only saved and restored forgets everything a branch learned. That is
safe but loses real information: after `if (x === null) { x = "d"; }`, `x` is a string
on every path, and a restore-only design reports it as `string | null`.

`check_if_statement` therefore walks each way through the statement from the state the
condition gives it, keeps the state each way ends in, and decides the state after the
statement from which ways can actually fall out the bottom:

| Consequent exits | Alternate exits | State after |
| --- | --- | --- |
| yes | no | the alternate's end state (the guard clause: `if (x === null) return;`) |
| no | yes | the consequent's end state |
| no | no | the join of the two end states |
| yes | yes | the state from before (the code after is unreachable) |

With no `else`, the missing alternate is the condition's false side alone.

`join_states` is the one operation behind this. A symbol narrowed on both paths becomes
the union of the two narrowed types. A symbol narrowed on only one path is dropped,
because the other path still holds whatever it had before, and the union of that with
the narrowed type is no narrower than before. Dropping is therefore always safe and
never wrong; it only gives up precision. The same function serves the false side of
`a && b`, the true side of `a || b` and the exit of a loop.

## 11. Discriminated unions: narrowing the parent by one property

`shape.kind === "circle"` narrows `shape`, not `shape.kind`. The narrowing key stays a
`SymbolId`: the handler filters the members of `shape`'s union by whether their `kind`
property can be that literal. Equality keeps members whose property type can equal the
literal (the exact literal, or a wider type that contains it). Inequality removes a member
only when its property is exactly that one literal, since a property typed `"a" | "b"`
could still be `"b"`. A member that is not an object, or has no such property, is kept:
this cannot decide it, and a kept member can only cost precision.

A lone object type, as opposed to a union, is returned unchanged.

A property tested by truthiness narrows the same way: `if (result.ok)` keeps the members whose `ok` can be truthy, and the `else` side keeps those where it can be falsy. A boolean discriminant (`ok: true` against `ok: false`) is therefore narrowed without a comparison. A plain `number` or `string` property is neither always truthy nor always falsy, so it never removes a member.

## 12. `switch`

`switch (k)`, `switch (shape.kind)` and `switch (typeof x)` narrow the variable in each
case body, using the same per-member filters as `if`.

- **Grouped labels** (`case "a": case "b": body`) fall into the next body, so the body is
  narrowed to the union of its labels, not just the last one. If any label in the group
  cannot be resolved to a literal, nothing is narrowed.
- **`default`** is the complement of every other case's test. A test that cannot be
  resolved is left out of the exclusion, which keeps more members than strictly needed
  and is always safe. A `default` grouped with other labels can be reached with any value
  and narrows nothing.
- Each case starts from the state the switch started with, not from the previous case's
  end state. Cases are checked in textual order, and without the reset one case's guard
  clause would narrow the next case's code and escape the switch.
- Fallthrough out of a non-empty body is not modelled: each such case is narrowed as if
  reached directly, which matches `tsc` for the usual `break` / `return` style.

## 13. `&&` and `||`

A condition can itself be a logical expression. `a && b` is true when both hold, so its
true overlay is `a`'s true overlay extended by `b`'s, with `b` narrowed against the state
`a` establishes (it only runs when `a` held). It is false when `a` was false, or when `a`
held and `b` did not, so its false overlay is the join of those two paths. `a || b` is the
mirror image. `??` narrows nothing here.

This is what makes `if (a === null || b === null) return;` leave both variables non-null
afterwards: the false side of an `||` is the extension of both false sides, not a join.

## 14. `in` and `instanceof`

`"radius" in shape` keeps the union members that have, or may have, the property. The
false side drops a member only where the property is required, since an optional or
missing property can still be absent. A lone object, or a union where no member has the
property, is left alone, because `tsc` would intersect with a record of the key and this
checker cannot express that.

`pet instanceof Dog` keeps the members assignable to the class's instance type, so a
subclass counts as its parent. A member that is a supertype of the class (declared
`Animal`, tested against `Dog`) becomes the class on the true side. Types are structural,
so two classes with identical shapes cannot be told apart by `instanceof`, in `tsc`
either. `any` and `unknown` become the class on the true side only; the error type and
generic parameters are left alone on both.

## 15. Literals, `unknown` and non-union types

Equality against a literal narrows a wider member to the literal: a lone `string`
compared with `"a"` is `"a"` in the true branch, `unknown` becomes the literal, and
`boolean` is treated as `true | false`, so excluding `true` leaves `false`. Before this,
a lone non-union type compared against a literal collapsed to `never`. That is the
dangerous failure mode of narrowing: `never` is assignable to everything, so the bug
produces no error at all. The fixtures that pin this behaviour assign the narrowed value
to an incompatible type and expect exactly one error, which is the only way a `never`
would show up.

Loops follow the same overlay rules. A loop test narrows the body. After the loop the state
is the join of the state from before and the body's end state. If nothing in the body can
`break`, the loop is only left by its test failing, so the test's false side also holds
afterwards: after `while (x !== null) { ... }`, `x` is null. The `break` scan is
deliberately generous (a `try` is assumed to hide one), because a false "there is a
break" only skips a narrowing.

## 16. Still not narrowed

- Property paths. `a.b.kind === "x"` and `a.b !== null` need the narrowing key to become a
  path instead of a `SymbolId`, which is a change to `NarrowState`, not another handler.
- User-defined type predicates (`x is T`) and assertion functions. These need a predicate
  on function types.
- Optional chaining (`x?.kind === "a"`), `switch (true)`, equality between two variables.
- A join after `switch`. After the switch the state is the one from before it.
- Captured variables. A closure starts with a copy of the surrounding narrowing, which is
  right for a variable that is never reassigned and wrong for one that is.
- Loop fixed points. The body is walked once.
## 17. Added after the first write-up
- **A declared union narrows to its initializer.** `const w: Wide = "variant0"` is the literal on the next line, as in tsc, and the wide-union benchmark uses parameters for that reason (commits `21641e3` and `6d87fce`).
- **Narrowing through an assignment.** `(x = f()) !== null` narrows `x` (`21641e3`).
- **More statements follow control flow.** `do-while`, `for-of`, `for-in`, `try`, `throw` and labeled statements are walked, with loop and try narrowing following the same overlay rules as sections 10 and 15.
- **A property read on a nullable value reports the cause.** `x.length` on `string | null` is `'x' is possibly 'null'` (TS18047, 18048 for `undefined`, 18049 for both), named as written (`user.address`), or `Object is possibly 'null'` (TS2531, 2532, 2533) when there is no name. The property is then looked up on what is left, so a missing property is still reported, and an optional chain is left alone (`891b176`). This is a diagnostic rather than a narrowing rule, but it uses the same non-nullish type narrowing produces.
