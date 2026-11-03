# tsox vs tsc 6.0.3 — corpus parts 01, 02, 03, 04, 05 (`--module`, 500 files each)

Command per part: `./scripts/compare-local.sh tests/typescript-testcases/typescript-6.0.3/part-NN --module --runs 1 --timeout 90 --json`

## Hangs found (tsox does not finish)
- part-02: `compiler__chainedSpecializationToObjectTypeLiteral.ts` — tsox ran past 90 s (also times out alone at 3 s). Excluded from the part-02 numbers (499 files).
  Trigger: `Sequence<T>` with `groupBy<K>(...): Sequence<{ key: K; items: T[] }>`, then `s.groupBy(...)` and `.each(x => x.key)`.
- part-05: `compiler__genericClasses4.ts` — tsox took 61.6 s for the whole part (others ~0.2 s); alone it exceeds 3 s. Included in the numbers.
  Trigger: `class Vec2_T<A>` with `apply<B>(f: Vec2_T<(a: A) => B>): Vec2_T<B>` (file comment: "once caused stack overflow").
Both look like generic instantiation that grows each level, the same family as `tests/lib-probe-hangs/` (not confirmed).

Notes: counts per code are lines (a file can contribute several); examples are the first message seen per code.
tsox build: working tree with uncommitted `typeof` change (not mine). `--timeout 5` stops tsc (about 6 s per 500-file part), so tsc used `--timeout 90`.

## Per part
| part | MATCH | MSG DIFF | CODE DIFF | GAP | FALSE POS | MIXED | agree lines | exact lines | tsox ms | tsc ms |
|---|---|---|---|---|---|---|---|---|---|---|
| 01 | 207 | 3 | 23 | 252 | 6 | 9 | 233/500 | 259/996 | 144.3 | 6087 |
| 02 | 240 | 3 | 19 | 205 | 23 | 9 | 262/499 | 429/1639 | 177.2 | 6805 |
| 03 | 291 | 4 | 11 | 152 | 32 | 10 | 306/500 | 157/783 | 235.1 | 6127 |
| 04 | 227 | 14 | 6 | 221 | 23 | 9 | 247/500 | 138/1158 | 157.4 | 7475 |
| 05 | 235 | 14 | 22 | 199 | 19 | 11 | 271/500 | 258/1031 | 61624 | 5834 |
| **all** | 1200 | 38 | 81 | 1029 | 103 | 48 | | | | |

## Gaps by tsc code (errors tsox misses), top 15
| code | count | example |
|---|---|---|
| TS2454 | 269 | Variable 'a' is used before being assigned. |
| TS2322 | 261 | Type 'null' is not assignable to type 'Result'. |
| TS7006 | 232 | Parameter 'x' implicitly has an 'any' type. |
| TS2304 | 226 | Cannot find name 'B'. |
| TS7005 | 180 | Variable 'x' implicitly has an 'any' type. |
| TS18048 | 163 | 'T' is possibly 'undefined'. |
| TS2339 | 130 | Property 'collection' does not exist on type '{ readonly primaryPath: any; }'. |
| TS1005 | 102 | ',' expected. |
| TS2300 | 99 | Duplicate identifier 'myFn'. |
| TS2345 | 85 | Argument of type 'symbol' is not assignable to parameter of type 'object'. |
| TS1240 | 84 | Unable to resolve signature of property decorator when called as an expression. |
| TS2367 | 69 | This comparison appears to be unintentional because the types '0' and '1' have n |
| TS7008 | 67 | Member 'var' implicitly has an 'any' type. |
| TS2564 | 65 | Property 'a' has no initializer and is not definitely assigned in the constructo |
| TS7031 | 60 | Binding element 'foo' implicitly has an 'any' type. |

## False positives by tsox code (wrong errors), top 15
| code | count | example |
|---|---|---|
| TS2339 | 181 | Property 'collection' does not exist on type '{ readonly primaryPath: any; }'. |
| TS2322 | 118 | Type 'null' is not assignable to type 'Result'. |
| TS2345 | 85 | Argument of type 'symbol' is not assignable to parameter of type 'object'. |
| TS2344 | 15 | Type 'GetProps<C>' does not satisfy the constraint 'Shared<TInjectedProps, GetPr |
| TS18048 | 11 | 'T' is possibly 'undefined'. |
| TS7008 | 11 | Member 'var' implicitly has an 'any' type. |
| TS2488 | 10 | Type 'number' must have a '[Symbol.iterator]()' method that returns an iterator. |
| TS2554 | 10 | Expected 0 arguments, but got 3. |
| TS2349 | 9 | This expression is not callable. |
| TS7053 | 8 | Element implicitly has an 'any' type because expression of type '"arguments"' ca |
| TS18047 | 5 | 'x' is possibly 'null'. |
| TS2739 | 3 | Type '{ a: error; }' is missing the following properties from type '{ a: string; |
| TS2351 | 3 | This expression is not constructable. |
| TS2362 | 2 | The left-hand side of an arithmetic operation must be of type 'any', 'number', ' |
| TS2304 | 1 | Cannot find name 'B'. |

## Exact matches, top 8
| code | count | example |
|---|---|---|
| TS7006 | 325 | Parameter 'x' implicitly has an 'any' type. |
| TS2564 | 230 | Property 'a' has no initializer and is not definitely assigned in the constructo |
| TS7010 | 149 | 'foo', which lacks return-type annotation, implicitly has an 'any' return type. |
| TS2322 | 149 | Type 'null' is not assignable to type 'Result'. |
| TS7008 | 63 | Member 'var' implicitly has an 'any' type. |
| TS2339 | 45 | Property 'collection' does not exist on type '{ readonly primaryPath: any; }'. |
| TS2304 | 42 | Cannot find name 'B'. |
| TS2345 | 41 | Argument of type 'symbol' is not assignable to parameter of type 'object'. |


## Analysis notes (added when saved as baseline)

Arithmetic from the table above, 2,499 files: MATCH 1200 (48%), GAP 1029 (41%), FALSE POS 103 (4%).
Reported separately as 40% / 51% / 7%; reconcile which run that came from.

Top-15 gap lines: 2,092.
- tsc codes with no tsox variant (764 lines, 37%): TS2454 (269), TS7005 (180), TS1005 (102),
  TS1240 (84), TS2367 (69), TS7031 (60).
- Codes tsox can emit but misses (about 1,300 lines): TS2322, TS7006, TS2304, TS18048, TS2339, TS2300.

Not done: per-file ranking (files unlocked per code) and a count of 9000-series
"Unimplemented*" / "Unresolvable*" warnings. Both need the per-file --json output.
