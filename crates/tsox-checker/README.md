# tsox-checker

**Layer 2.** The single-file checker. Depends on `tsox-base` and `tsox-types`.

- `bridge/`: parse and bind with Oxc, declare top-level signatures, then check bodies (`statements/`, `expressions/`, `narrow`) and run the whole-file passes. `check_file` is the entry point.
- `unit`: a file as a unit of work. A `Worker` owns an arena, checks a `UnitContext`, and returns a `UnitOutput` (diagnostics, exports, portable declarations). A panic or cancellation yields an `incomplete` output, never a wrong one.

It knows nothing about the module graph, scheduling or Salsa.
