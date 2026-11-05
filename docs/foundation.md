# Foundation: Initial Type-Checking Foundation

> **Architecture case study:** how tsox started as a small, explicit checker pipeline before introducing richer type semantics.

## 1. What this stage was trying to prove

Foundation was not trying to implement TypeScript in one jump. The goal was to prove a clean end-to-end path: TypeScript source -> Oxc parser/semantic layer -> tsox checking logic -> type compatibility -> diagnostics.

Fixtures: `primitive_ok.ts`, `primitive_mismatch.ts`, `function_arg_mismatch.ts`, `function_return_mismatch.ts`, `binary_op_mismatch.ts`, `unsupported_statement.ts`.

The key policy was honesty about unsupported syntax. Unsupported constructs should produce an explicit diagnostic instead of silently passing.

## 2. Architecture chosen

Oxc supplies expensive front-end infrastructure: parsing, AST, semantic symbols/scopes, and source spans. tsox owns the type semantics. The bridge keeps these concerns separated:

```text
Oxc AST + semantic data
        |
        v
crates/tsox-checker/src/bridge
  parse / declare / expressions / statements / context
        |
        v
semantic Type model
        |
        v
subtyping + diagnostics
```

This keeps tsox from becoming an Oxc fork while avoiding the cost and risk of rebuilding a parser and semantic analyzer.

## 3. Why an explicit `Type` model

`crates/tsox-types/src/types.rs` defines the semantic vocabulary: primitives, `Any`, `Unknown`, `Error`, functions, objects, arrays, unions, literals, `Never`, and generic parameters. The checker asks semantic questions against this model rather than against Oxc AST nodes.

This gives the type checker a stable internal language that can survive syntax changes and supports later inference, narrowing, and substitution.

## 4. Why `TypeId` + `TypeArena`

Types live in `TypeArena` and are passed around as compact `TypeId` handles. This is an arena/handle pattern.

It was chosen because type information becomes graph-shaped quickly. Handles make nested types cheap to pass, allow shared references, and provide a natural foundation for recursive and generic types without making ownership the central problem.

```text
TypeId ----> TypeArena entry
               |
               +--> Array(TypeId)
               +--> Union(Vec<TypeId>)
               +--> Function(... TypeId ...)
```

## 5. Why subtyping is centralized

`crates/tsox-types/src/subtyping.rs` owns `is_subtype(arena, sub, sup)`. Expression code determines what type an expression has; subtyping determines whether one type can be used where another is expected.

This separation prevents each expression handler from inventing compatibility rules and gives later features one reusable semantic relation.

## 6. Why diagnostics are a separate layer

`crates/tsox-base/src/diagnostics.rs` defines the public diagnostic representation. Compiler internals produce structured diagnostics, while tests, CLI code, and WASM can consume them without knowing how checking works internally.

## 7. Why explicit unsupported diagnostics

The unsupported-construct fixture is an architectural test. At Foundation the construct it used was a class, which the checker did not support, so it reported the construct instead of silently ignoring it. That is a conservative error-recovery philosophy, and it has stayed useful as the language subset grew: the fixture has been rewritten several times as the construct it used became supported (a class, then a do-while, a namespace, an ambient module and a global augmentation). It is now `unsupported_statement.ts`, a `with` statement, which is not allowed in strict mode or in a module and has no plan to be supported. Any statement that still reaches the unsupported path does the same job.

The same rule governs everything since: an unsupported construct is reported as unchecked (a `TSR90xx` warning) and not guessed at. [language-coverage](language-coverage.md) lists what is checked today and what is not.

## 8. Test-driven workflow

Each fixture is a semantic claim. The fixture test creates a fresh `TypeChecker`, checks the source, and asserts either no diagnostics or the expected diagnostic behavior.

```text
feature idea
    -> minimal fixture
    -> implementation
    -> regression test
```

The fixture is executable documentation.

## 9. What Foundation established

- Oxc as the front-end boundary
- tsox-owned semantic types
- `TypeId` + arena storage
- centralized subtyping
- structured diagnostics
- explicit unsupported-feature reporting
- fixture-driven regression testing

## 10. Stage contract

Foundation is the permanent regression contract for primitive compatibility, function argument/return checking, basic operator checking, diagnostics, and unsupported-feature reporting.
