# The Safety Net and Resource Limits

> **Architecture case study:** keeping a checker from aborting on hostile input, when a stack overflow cannot be caught and one bad file takes the whole run down, and when a limit that says so is better than an answer that is wrong.

This is the safety side of the [project engine](project-engine.md). It has two halves. The first sets a bar: no abort on an agreed corpus, and the same output however the threads run. The second is the work that bar does not cover: limits on nesting, recursion and instantiation, and the measurements that are still missing. They are one story, because both come from one fact: in Rust a stack overflow is not a panic.

## 1. How it evolved

| When | Change | Commit | Why |
| --- | --- | --- | --- |
| 2026-10-15 | Determinism hooks, a hash-seed test and a per-fixture abort script | `a1a90fa` | Output must not depend on which worker finishes first or on hash order, and a crash has to be found one file at a time |
| 2026-10-16 | The corpus abort test | `89a0621` | "Does any file abort the process" needed an answer inside `cargo test` |
| 2026-10-18 | Refuse files nested past 4,000 brackets before parsing them; the stack guard | `bdd82df` | The parser recurses once per level of nesting, and nothing in the checker can stop it |
| 2026-10-31 | Count the chains that recurse in the parser; stop deep statement nests | `7a40fae` | A ternary chain, an arrow chain and nested generics recurse as deeply as brackets do, and have no brackets to count |

## 2. What the corpus guarantee promises, and what it does not

**Plan.** The guarantee is no abort on the agreed corpus: the 10,083 files of the TypeScript test corpus and 8 known crashers, each checked in a process of its own with `RUST_MIN_STACK` unset. One file per process means a crash names its file. It also means the checker is tested on the stack a user gets, not on one a test harness enlarged.

**Evidence.** On `5789d5a` (2026-10-18) the whole corpus and the 8 crashers ran with no abort: about 215 s as a debug test, and about 135 s with the release binary and `scripts/isolate-fixture-aborts.sh`. The determinism test, [`crates/tsox-cli/tests/determinism.rs`](../../crates/tsox-cli/tests/determinism.rs), checks that the output is byte-identical across thread counts. A hook compiled only into unit tests changes the order workers finish in without changing what they read, so a test can show that reports do not depend on it. Production builds have no delay branch and no environment knob.

**What it means.** No abort *on this corpus*. It does not mean that no input can abort. The rest of this document is about the inputs the corpus does not hold.

**A trap in the corpus test.** `corpus_aborts` runs one file in 500 by default, a fixed sample of about twenty that takes seconds. `TSOX_CORPUS_STRIDE=1 cargo test --test corpus_aborts` runs all of them. The variable used to have another name; the old name is ignored without a warning, so a run that still sets it silently covers only the default sample.

## 3. Why an overflow is the enemy

**Problem.** The checker walks expressions and type declarations by recursion, and so do the parser and the semantic builder it sits on. An input nested deeply enough overflows the thread's stack. That is an abort, not a panic: the `catch_unwind` around each worker cannot see it, and one file loses the whole run. Everything else that can go wrong in a file is contained by that `catch_unwind` and turned into an incomplete result; an overflow is the one failure it cannot contain.

So three questions decide the design. Can the file be refused before anything recurses? Can our own recursion stop itself? Is the stack at least as large as the deepest thing we will let through?

## 4. Three layers, each covering what the one before cannot

| Layer | What it does | What it covers | What it cannot do |
| --- | --- | --- | --- |
| A deeper stack | Workers and discovery run on 64 MB | Ordinary deep code, the gap between the main thread and spawned threads | Remove the limit; it only moves it |
| The stack guard | The checker's own recursion asks if the stack is nearly used up | Our walks | Reach the parser, which recurses on its own |
| Refuse before the parse | One pass over the bytes counts nesting and refuses a file past a limit | The parser's recursion | Be exact: it is a count, not a parse |

### 4.1 A stack deep enough for the workers

**Problem.** A project run aborted on one file that is a single chain of about 1,500 `+` operators (TypeScript's own `binderBinaryExpressionStress`). It parses into a left-nested tree 1,500 levels deep, and walking it recurses once per level inside Oxc's visitors, about 16,000 frames in the backtrace. That code is not ours to make iterative. The same file passed when it was the only one checked, because that runs on the main thread, whose stack is about 8 MB. Rayon's workers are spawned threads, and a spawned thread gets about 2 MB by default.

**Picked.** Build the workers with a 64 MB stack. It is address space that is only touched where a file really recurses deeply, so ordinary runs use no more memory. Graph discovery and the CLI's flat mode run on the same size, so a file discovery can parse is a file a worker can parse. The old workaround had been to export `RUST_MIN_STACK` before running.

**Cost.** A chain several times longer would still overflow. Fixing that for good needs a depth limit in the parser or an iterative walk, and both are Oxc's to provide.

### 4.2 A guard that reads the stack pointer

**Picked.** A thread that is going to run checker code says how big its stack is, and the recursive entry points ask `exhausted()` before going one level deeper (`crates/tsox-base/src/stack_guard.rs`). The answer is by address and not by a depth count, because one level costs very different amounts in a debug and a release build, and in an expression walk and a type walk. The address of a local is as near the stack pointer as safe code can get. It needs no dependency and no `unsafe`.

**Covers.** `infer_expression_type`, `TypeNamespace::resolve`, `resolve_ts_type`, `check_statement` and the stable hasher. Past its budget, a file gets one `TSR9102` warning and the rest of it is left unchecked, or an export is marked incomplete. Never wrong, only less complete.

**Cost.** A thread that never declared its stack size is not guarded, which is every caller that has not opted in. And the guard covers our own recursion only. Because it works by address, an extreme input can be cut at a different depth in a debug build than in a release build; on ordinary code the output is unaffected (byte-identical to the previous binary on 2,000 corpus files, with no false `TSR9102`).

### 4.3 Refuse the file before the parser sees it

**Tried, and why it was not enough.** A guard in the checker's own walks cannot reach the parser's. A bigger stack moves the limit and does not remove it.

**Picked.** `crates/tsox-base/src/source_depth.rs` counts the nesting of `(`, `[` and `{` in one pass over the bytes before the parser sees the file, and refuses a file nested past `MAX_BRACKET_DEPTH` (4,000) with an error that says so. The count skips comments, strings and template literals, and uses a counter and not recursion, so it is safe at any depth. It is used before every parse: in the checker, and in the walk that finds a file's imports. On a release build every bracketed shape tried, from parentheses to nested classes, was refused at 10,000 and at 100,000 levels with no abort. The limit leaves a factor of about five under the point where a debug build aborts on the worst construct measured (nested objects, about 20,000 levels), and real code nests a few dozen levels.

**Extended.** Three shapes recurse in the parser as deeply as brackets do and have none to count: a conditional chain (`c ? 1 : c ? 2 : ...`), an arrow chain (`y => y => ...`) and nested generic arguments (`Array<Array<...>>`). A `?` that starts a conditional and a `=>` each add one to a chain counter, and a `<` that follows a name adds one to an angle counter until a `>` takes it off, so the answer is how far the parser would recurse. `crates/tsox-cli/tests/depth_guard.rs` runs generated nests of each through the real binary.

**Cost.** It is a count, not a parse. A regular expression literal that holds a quote or a backtick can confuse it for the rest of the line, hiding some nesting or adding some. A `<` that is a less-than counts like an opening generic until the next `;`, so thousands of comparisons in one statement would be refused, which real code does not contain. Flat chains the count does not follow (`a + b + ...`, `o.a.a.a`, a call chain `o().a().a()`) can still abort a debug build at default stacks, at 100,000 to 200,000 terms; the module's own note says none of them is code anyone writes.

### 4.4 Instantiation depth

Substituting type arguments can recurse without end on a generic that refers to itself. `MAX_INSTANTIATION_DEPTH` is 100; past it the result is cut and the file gets the `TSR9102` warning once. tsc reports this as TS2589, which needs a source position; that is not done.

## 5. Limits that remain, measured

These came out of the corpus and depth-guard work. None of them changes a decision above.

- **Oxc's parser has no depth limit of its own.** Before the bracket limit, a ternary chain, an arrow chain and nested generics overflowed it at roughly 44,000 to 63,000 levels in a release build. The counts in section 4.3 cover them now; a limit from Oxc would remove the need for the counts.
- **In a debug build, Oxc's semantic builder overflows on flat chains** (member access, `+`, `?.`, array suffix) at about 50,000 levels, before the guard runs. A release build is cut cleanly at 100,000. The depth tests are sized for this.
- **A chain of `else if` branches that fall through is cubic.** Release build:

  | Branches | Time |
  | --- | --- |
  | 500 | 0.27 s |
  | 1,000 | 2.0 s |
  | 2,000 | 15 s |
  | 4,000 | over 120 s |

  Chains of `return` branches and guard clauses stay fast. Each level joins its branch with the join of everything below it, so the joined union grows by one member per level, and `alloc_union` removes duplicates pairwise, which is quadratic per union. A hash or id-first dedupe, and skipping the union when both sides are the same type, should fix it. Chains of this length are rare in real code.
- **A chain of `class A extends B` is quadratic:** 1.4 s, 5.2 s and 34 s at 5,000, 10,000 and 20,000 levels, on the old build as well as the new one.
- **A loop that assigns a narrowed variable is walked up to four times**, and nested loops multiply that. This comes from a comment in the code and has not been measured.

## 6. What is still not done

- Budgets for recursive work, instantiation and memory are undefined. Today there are depth limits and no budget of work.
- The 100,000-level run of statement recursion on a release build has not been repeated since `check_statement` got the guard.
- Miri or ASan on the arena and reset paths, and a compile-fail ownership check if the arena design allows one, have not been done (neither tool was installed on the machine used; the checker has no `unsafe`). The three uses of hash-map iteration flagged in output paths are annotated as order-independent.
- The checker records no metrics of its own (time, allocations and memory per phase, how often a query re-runs). The harness and the benchmarks measure from outside.
- Whether the `alloc_union` fix is worth doing now or waits for the budgets work is undecided.

## 7. Invariants to keep

1. **A limit reports; it never guesses.** Past a limit the file or the export is marked unchecked or incomplete, with `TSR9102` or an error that says so. It never produces a type that is wrong.
2. **An abort is a bug.** Any input that aborts the process is a failure, even one nobody writes. The corpus test, at stride 1, is the check.
3. **Output does not depend on timing.** Reports are the same at any thread count and any completion order.
4. **A guarded thread declares its stack.** A new thread that runs checker code calls `set_thread_stack`, or it is not protected.
5. **The refusal comes before the parse.** Nothing that can recurse unbounded runs on a file the nesting count has not seen.
