# Literal Widening: Literal Types and Widening

> **Architecture case study:** adding value-sensitive types without breaking the semantic type model.

## 1. Why this milestone existed

Primitive types alone cannot model `const` literal preservation or literal unions. Literal Widening introduced string, number, and boolean literal types plus widening.

Fixtures: `const_keeps_literal_type.ts`, `let_does_not_keep_literal_type.ts`, `let_widens_to_primitive.ts`, `array_literal_elements_widen.ts`, `literal_union_annotation.ts`, `literal_union_rejects_unlisted_value.ts`.

## 2. Explicit literal variants

`Type` gained `StringLiteral`, `NumberLiteral`, and `BooleanLiteral` rather than attaching literal metadata to primitive variants.

This makes specificity explicit:

```text
"red"  <  string
"red" | "blue"  !=  string
```

The same subtyping machinery can then reason about literals and primitives.

## 3. Why widening is a separate operation

`widen(arena, type_id)` centralizes conversion from literal types to primitive types. This avoids destroying literal information too early.

```text
literal type
   |
   +-- preserve when semantics need specificity
   |
   +-- widen when mutable/inferred context requires it
```

## 4. `const` versus `let`

The implementation applies widening at variable-type establishment rather than changing the primitive representation itself. `const` can preserve a literal while `let` widens toward its primitive type.

Array literal element widening is covered separately so mutable arrays do not accidentally become permanently literal-valued structures.

## 5. Literal unions compose with existing unions

No special `LiteralUnion` type was introduced. A literal union is simply a normal union containing literal members. This is an example of the existing architecture absorbing a feature without a new semantic subsystem.

## 6. Why this matters for narrowing

Literal unions provide the raw semantic material for later discriminated-union and control-flow analysis. Literal Widening therefore prepares the model used by Control-Flow Narrowing.

## 7. Architecture lesson

The main pattern is **semantic normalization through explicit operations**. Literal-specific behavior is centralized in `widen` instead of being reimplemented by each caller.

## 8. What Literal Widening completed

- string/number/boolean literal types
- literal unions
- `const` literal preservation
- `let` widening
- array literal widening
- a reusable widening operation
