# Vendored TypeScript lib files

`typescript-6.0.3/` holds all 108 `lib*.d.ts` files of TypeScript 6.0.3 (4.0 MB), unmodified:
byte-identical to `typescript/lib/` in the npm package that `scripts/ts-diag-tool` installs.
They are Microsoft's, under the Apache License 2.0. The default ES2020 set is 51 of them; the
rest are `es2021`..`esnext`, `webworker` and `lib.d.ts` (the ES5 default), which the corpus asks
for through `@target esnext` / `@lib` (about 6% of fixtures).

`<target>.order.json` is the order a target loads its files in: each file's
`/// <reference lib>` dependencies before it, depth first. There is one per target: `es5`
(`lib.d.ts`), `es2015`..`es2025`, `esnext` and `webworker`. `es2020.order.json` is the default.
They are produced by `scripts/ts-diag-tool/gen-lib-order.js` (it reproduces the original
`es2020.order.json` exactly): `node gen-lib-order.js typescript-6.0.3 esnext.full`.

## Where they live, and why

They are data, so they sit in `tsox-base`, beside `lib_global_types.rs` (the list of global
names, which comes from these files), and every crate above can read them. Nothing embeds them
yet: `include_str!` of 2.8 MB would grow the wasm bundle, so the loader (a later step) decides
whether to embed, compress or read them lazily by name.

## What they are for

The checker has no real lib yet: global names come from a list and the types of `Promise`,
`Record`, `Array` methods and the like are hand-written. The plan is to check these files once,
as a provider module (`ModuleKey::builtin()`), merge their global interfaces, suppress their
diagnostics, and load them lazily by name. `tests/fixtures/lib-probe/` holds one small fixture per
construct these files use that the checker cannot resolve yet (`l01`..`l14`), and
`tests/lib-probe-hangs/` the two inputs that still hang.
