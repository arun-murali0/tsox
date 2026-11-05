# Parameters and Destructuring

> **Architecture case study:** extending binding and call semantics without creating a second type-checking path.

## 1. Why Parameters and Destructuring is a separate milestone

Parameters and Destructuring focuses on how values enter functions and local bindings: optional parameters, rest parameters, rest bindings, object destructuring, renamed bindings, array destructuring, defaults, and destructured function parameters.

There are 13 fixtures in `tests/fixtures/parameters-destructuring/`.

## 2. The existing semantic model was enough

`Param` already stores:

```rust
pub struct Param {
    pub type_id: TypeId,
    pub optional: bool,
    pub rest: bool,
}
```

Parameters and Destructuring makes that metadata operational instead of creating new parameter-specific type categories.

## 3. Optional parameter arity

The call checker distinguishes required, optional, and rest parameters. An optional parameter may be omitted, but it does not make preceding required parameters optional.

```text
parameter metadata
       -> required-count / max-count
       -> call arity validation
```

This is why `optional_param_does_not_excuse_missing_required_arg.ts` exists.

## 4. Rest parameters

A rest parameter permits arbitrary trailing argument count, but each supplied argument is still checked against the rest element type.

The semantic distinction is:

```text
incoming arguments: T, T, T, ...
bound identifier:  Array<T>
```

`rest_param_identifier_is_bound_as_array.ts` protects the second rule.

## 5. Destructuring is a binding operation

The statement layer routes destructuring through reusable helpers:

```text
check_destructured_declarator
          |
          v
     bind_pattern
       /     \
 object       array
 pattern      pattern
```

This abstraction separates pattern shape from assigning semantic types to bindings.

## 6. Object destructuring

For `{ name } = user`, the checker resolves the source object, looks up the property, and binds the local symbol to that property's `TypeId`. Missing properties remain diagnostics.

Renamed bindings such as `{ name: displayName }` keep the source property name separate from the local binding name.

## 7. Array destructuring

For `[first] = values`, an `Array<T>` source produces `first: T`. A non-array source is diagnosed. This directly reuses the existing array representation rather than introducing a destructuring-specific type.

## 8. Defaults

Destructuring defaults are semantic expressions. The checker does not ignore them merely because the parser has already accepted the syntax. The default expression participates in type checking.

## 9. Function parameters reuse the same binding path

A destructured function parameter uses the same pattern-binding machinery as a variable declaration. This is an important reuse boundary:

```text
variable declaration ----                          -> bind_pattern -> symbols/types
function parameter ------/
```

The checker therefore avoids two independent implementations of destructuring semantics.

## 10. Interaction with Oxc symbols

Bindings ultimately enter `SymbolTypeMap` using Oxc semantic `SymbolId` values. That means destructured locals automatically participate in ordinary identifier lookup and later flow narrowing.

```text
pattern
  -> binding identifier
  -> Oxc SymbolId
  -> SymbolTypeMap
  -> TypeId
```

## 11. Why no destructuring type system

Destructuring consumes existing semantic types. Objects remain `ObjectType`; arrays remain `Array(TypeId)`; locals remain `SymbolId -> TypeId`.

Only the syntax of the binding changes.

This is a strong example of the project's main design principle: add language behavior by composing semantic primitives.

## 12. What Parameters and Destructuring completed

- optional parameter arity
- rest parameter count and type checking
- rest identifier array binding
- object destructuring
- renamed object bindings
- missing-property diagnostics
- array destructuring
- destructuring type errors
- defaults
- destructured function parameters

The architecture lesson is: **binding syntax should feed one semantic binding pipeline.**
