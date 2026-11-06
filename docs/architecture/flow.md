# The flow graph

`tsox_checker::flow` builds, for every function body and the top level of a file, a table of
flow nodes in the shape tsc's binder produces. It is arena-free data and a pure function of the
source, so it fits the query layer like the other unit outputs. Nothing reads it for narrowing
yet; see "Phases" below.

## Hybrid: what comes from where

| From `oxc_cfg` (the skeleton) | From one AST pass (what the skeleton lacks) |
|---|---|
| Basic blocks and edges | Which successor is the true one and which the false one: the edge into an `if`, ternary or loop body, or into the right operand of `&&`, `\|\|`, `??` |
| `Finalize` and explicit `Error` edges (try, catch, finally) | Assignments: declarators, `x = v`, `x++`, `for (x of …)`, destructuring, `a.b.c = v` |
| The node-to-block map (`nodes().cfg_id`), used for every AST node | Switch dispatch: the block each `case` is entered by, and what falling through means |
| Block order inside a body | Where an expression ends when it branches (`a ?? b` ends in the block where its paths meet) |

Back edges are found by a depth-first walk, not read from oxc: oxc marks the closing edge of a
`while` or `for` as `Backedge` but a `continue` as an ordinary jump.

## Reachability: the one place the skeleton is not used

"Is this statement dead" is answered by `crate::reachability`, not by oxc's `is_unreachable`.
oxc builds a correct graph of JavaScript, and tsc's binder builds a different one, and every
difference changes which statements tsc calls dead:

- a condition is constant only when it is the bare `true` or `false` keyword; `(true)`, `!true`,
  `1` and `null` are not, while `&&` and `||` carry constants through their operands and `??`
  does not;
- `break` and `continue` jump straight to their target, past an enclosing `finally`;
- a `catch` and a `finally` are entered from the start of their `try`, so they are live whenever
  the `try` is;
- a `switch` with no `default` always falls out of the bottom;
- a diagnostic covers a run of adjacent dead statements, and a function declaration, an empty
  statement, a `var` without initializer or a block ends the run.

`reachability::analyze` replays those rules over the AST and returns the dead statements and
the TS7027 / TS7029 spans. It is a pure function of the program, so definite assignment and the
narrowing resolver can ask the same question and get the same answer. The graph stays what it
is good for, blocks and the node-to-block map, for the dataflow phases below.

Still open here because they need types: a call to a `never`-returning or `asserts false`
function, and a `switch` that exhausts its discriminant. tsc folds both into the same question
when it checks.

## Flow nodes

`Start`, `Join`, `LoopHead`, `Assign { target, at }`, `Condition { test, assume }` and
`Clause { discriminant, matched, includes_default, all_tests }`. A node lists its antecedents,
each with an edge kind (`Normal`, `Back`, `Finalize`, `Exception`). Every `Condition` and
`Clause` sits on an edge, so a path that skips an operand does not carry that operand's
condition. An `if` test is carried whole on both of its edges (`narrow_condition` already
knows how to take `a && b` apart), and the operands of `&&`, `||`, `??` carry their own on the
edge into the right operand.

`BodyFlow::references` lists every read of a name with the flow node it is read at, so the
question "what is the type of this reference" is answered by walking antecedents backwards from
one node, and only for the references that are asked about.

## Identity

An expression is an `ExprRef`: the AST-order index of its node minus that of the body's root.
A comment or blank line above a function does not change its graph (tested). A symbol is its
index in the file's symbol table plus its name; adding a declaration earlier in the file
renumbers symbols, so the query layer should key on the name when it needs stability.

## Not covered yet

Optional chains do not split the graph. Class property initializers and static blocks have no
body of their own. A call is not an event, so assertion functions and never-returning calls are
missing. A declaration without an initializer is not an event, so definite assignment is
missing. A `catch` block is entered from the end of the `try` block only, not from every point
in it. These are the F3 items of the plan.

## Phases

1. **F1 (this change):** the table and its tests. No behaviour change.
2. **F2:** a backward, lazy resolver over the table runs beside the forward overlay in
   `bridge::narrow`; any disagreement is reported, and nothing is cut over until the
   difference is empty on the fixtures and the corpus.
3. **F3:** definite assignment, implicit return, exhaustiveness, assertion functions, catch
   entry from every point of the `try`.

## Checking it

`cargo test -p tsox-checker --lib flow::` runs the hand-written cases against
`crates/tsox-checker/tests/flow_fixtures/`, and the invariants (valid ids, a node without an
antecedent is a `Start`, no cycle without a back edge) over every file in `tests/fixtures/`.
The same invariants over a larger corpus:

```text
TSOX_FLOW_CORPUS=tests/typescript-testcases cargo test -p tsox-checker --lib flow::tests::corpus -- --ignored --nocapture
```

`TSOX_FLOW_FILE=x.ts cargo test -p tsox-checker --lib flow::tests::show -- --ignored --nocapture`
prints one file's graph.

## Checking reachability against tsc

`tests/reachability_rules.rs` pins the exact lines tsc reports for one case per rule above.
`scripts/flow-stress/run.sh` generates random control flow and compares tsox with tsc line by
line:

```text
scripts/flow-stress/run.sh --seed 1 --files 400 --codes 7027,7029
```

It prints, per code, how many lines agree and differ, then the statement around the first
differences. Any seed should report 0 differ.
