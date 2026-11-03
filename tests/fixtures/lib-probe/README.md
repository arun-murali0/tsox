# lib-probe fixtures

Small fixtures for the constructs the real `lib.*.d.ts` files use and tsox does not yet resolve
(measured on TypeScript 6.0.3 libs, default ES2020 set). Each marks its expected errors with
`// error`; real tsc is the reference.

    ./scripts/compare-local.sh tests/fixtures/lib-probe

| Fixture | Lib pattern |
|---|---|
| l01, l02, l03 | `declare var X: { prototype; new() }`, `declare var Array: ArrayConstructor`, `declare var JSON: JSON` |
| l04, l05, l09 | `K extends keyof T`, `keyof any`, `T[keyof T]`, keyof-keyed overloads (DOM `addEventListener`) |
| l06 | `T extends abstract new (...args: any) => any` (ConstructorParameters) |
| l07 | a second `namespace N` block naming types from the first (the `Intl` errors) |
| l08 | one unresolvable member must not poison the rest of the interface |
| l10 | `PromiseLike.then`: generics with defaults |
| l11 | `ReturnType`: constrained conditional type with `infer` |
| l12, l13, l14 | `Readonly`, `Uppercase`, `Array.map` callbacks |

The hang repros are in `tests/lib-probe-hangs/`, outside `tests/fixtures/`: the folder-walking
tests would not return.
