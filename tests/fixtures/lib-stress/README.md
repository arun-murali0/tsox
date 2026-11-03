# lib-stress

25 fixtures that use the standard library the way real code does, with no local redefinitions
of lib globals. They exist to measure and drive the lib work: each valid line must stay
error-free and each `// error CODE` line must report exactly that code.

Settings: `--strict`, target ES2020 (the vendored default set, `crates/tsox-base/lib/`).
Compare with `--flat --strict-only`.

tsc 6.0.3 is the oracle. `node scripts/ts-diag-tool/verify-markers.js tests/fixtures/lib-stress`
checks every marker against it (25/25 agree). Fix a fixture to match tsc, never the other way.

| file | lib area |
|---|---|
| s01-s06 | es5: Array, String/RegExp, Object/JSON/Math, Function bind/call/apply, utility types, typed arrays |
| s07-s14 | es2015+: Map/Set/Weak*, Promise/async, iterables/generators, Array/String/Object additions, Reflect/Proxy, bigint, Intl |
| s15-s17 | dom: elements and events, fetch/URL/storage/timers, globals and Error/Date |
| s18 | members newer than ES2020 (`at`, `hasOwn`, `replaceAll`, `Promise.any`, `WeakRef`, ...): tsc rejects them |
| s19-s23 | inference through lib signatures, subclassing builtins, async iteration, named groups, readonly/tuples |
| s24-s25 | decorators lib, errors that must not hide the correct ones around them |

## Baseline (tsox before the lib loader)

`compare.js --flat --strict-only`, 79 compared lines in 25 files:

- files matching tsc: 3 of 25 (s02, s11, and s24, which has no errors on either side)
- lines exact: 10 of 79; gap lines: 65 (tsc reports, tsox is silent); false-positive lines: 4
- gap codes: TS2322 x25, TS2345 x14, TS2550 x5, TS2339 x4, TS2540 x3, TS2769 x3, TS2554 x3
- why: 60x TSR9012 (annotation could not be resolved), 12x TSR9006 (expression kind not
  checked), 10x TSR9004 (computed member access), 6x TSR9008 (class shape), 2x TSR9001

The target for the loader is these 25 files at 25/25 with 0 false positives.
