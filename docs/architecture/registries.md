# Registries: where to add a rule

Each registry is a table of named rules, tried in order, so a new rule is one function plus one
row and does not touch the code around it. Every table has a test that its names are unique and
documented. Adding a rule never needs an edit to the driver.

| Table | File | Input | Output | Order matters when |
| --- | --- | --- | --- | --- |
| `PASSES` | `bridge/passes.rs` | the program, scoping, the check context | diagnostics appended to the context | always: the unreachable-code pass runs last (a test checks it) |
| `NARROWERS` | `bridge/narrow/narrowers.rs` | a condition expression | `Some((when_true, when_false))` or `None` | two rows could accept one node: the aliased-condition row is first |
| `EQUALITY_RULES` | `bridge/narrow/equality.rs` | an `==`/`===` (or negated) comparison | the pair of narrowing states, or `None` | the two optional-chain rows must come first (a test checks it) |
| `EFFECTS` | `bridge/narrow/effects.rs` | a discarded call (statement or comma part) | narrowing applied to the rest of the block | every answer is applied, in order |
| `CALL_FORMS` | `bridge/expressions/calls/forms.rs` | a call expression | the call's type, or `None` to fall through to overload resolution | a form accepts a callee shape another also accepts |

| `REPAIRS` | `bridge/syntax_repair.rs` | a parser message, the text and the error span | an edit that lets the rest of the file be checked, or `None` | the first repair whose `applies` is true decides, even if it answers `None` |

## Adding a rule

1. Write `fn my_rule(..) -> Option<..>` in the table's file. Return `None` for any shape you do not own.
2. Add a `{ name, doc, run }` row. The doc is one sentence stating the shape and the result.
3. Run `cargo test -p tsox-checker`: the table test checks the name and doc.
4. If the rule changes diagnostics, run the corpus parts that cover it and compare with tsc.
