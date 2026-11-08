# Gap checklist against tsc

Where tsox disagrees with tsc 6.0.3 on the 10k conformance corpus, by error code, so the work can be ordered by how many corpus lines it affects. Tick an item when its corpus count has gone down in a fresh run. Section 5 lists what the roadmap work has already moved.

- Measured: tsox `fb9ebd6` (release), tsc 6.0.3 through the API, **each of the 21 parts as its own project**, 10,081 files. Two files are left out because tsox does not finish on them (first two in section 4).
- Unit: a *line* is one file and line that has the code in tsc's output. *Missed* = tsc reports the code there and tsox does not. *Extra* = tsox reports it and tsc does not. *Right* = both report it.
- Files: exact match 4,284 (42.5%), of which 3,087 are clean on both sides; gap 4,128 (40.9%); false positive 685 (6.8%); mixed 373; code differs 437; message differs 174. Files agreeing on error lines: 4,895 (48.6%). Exact lines: 11,901 of 38,822 (30.7%).
- Diagnostics: tsc 45,542, tsox 22,936.
- Totals: 25,348 missed line-codes; 12,678 (50%) from 616 codes tsox never reports, 12,670 (50%) from 72 codes it reports only partly.

## Method: run the corpus part by part

An earlier version of this checklist ran the whole corpus as ONE project. That reference is wrong for some codes: tsc reported 322 TS2454 lines that way, and 5,798 when each part is its own project (and most of the 3,081 false-positive TS2454 lines in the single run are really agreement: per part, 3,052 lines right and 144 extra). Counts from the one-project run are not comparable with these. Why one project loses diagnostics is not understood; the corpus README already says to run one folder at a time.

## 1. Codes tsox reports only partly (completeness)

Highest yield: the check exists, so this is breadth. Sorted by missed lines.

- [ ] **TS2304** Cannot find name 'X'. — missed 1,828, right 942, extra 98
- [ ] **TS2322** Type 'X' is not assignable to type 'X'. — missed 1,461, right 1,586, extra 951
- [ ] **TS2454** Variable 'X' is used before being assigned. — missed 1,241, right 3,052, extra 144
- [ ] **TS7006** Parameter 'X' implicitly has an 'X' type. — missed 718, right 1,216, extra 121
- [ ] **TS2339** Property 'X' does not exist on type 'X'. — missed 611, right 429, extra 1,244
- [ ] **TS2362** The left-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum typ — missed 495, right 430, extra 17
- [ ] **TS2345** Argument of type 'X' is not assignable to parameter of type 'X'. — missed 485, right 333, extra 621
- [ ] **TS2564** Property 'X' has no initializer and is not definitely assigned in the constructor. — missed 463, right 1,397, extra 8
- [ ] **TS2300** Duplicate identifier 'X'. — missed 445, right 122, extra 7
- [ ] **TS2363** The right-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum ty — missed 386, right 393, extra 28
- [ ] **TS2365** Operator 'X' cannot be applied to types 'X' and 'X'. — missed 363, right 65, extra 28
- [ ] **TS4114** This member must have an 'X' modifier because it overrides a member in the base class 'X'. — missed 348, right 288, extra 51
- [ ] **TS18048** 'X' is possibly 'X'. — missed 284, right 72, extra 271
- [ ] **TS2532** Object is possibly 'X'. — missed 253, right 38, extra 7
- [ ] **TS2307** Cannot find module 'X' or its corresponding type declarations. — missed 227, right 105, extra 6
- [ ] **TS7008** Member 'X' implicitly has an 'X' type. — missed 203, right 637, extra 33
- [ ] **TS2683** 'X' implicitly has type 'X' because it does not have a type annotation. — missed 203, right 35
- [ ] **TS2769** No overload matches this call.
  Overload N of N, 'X', gave the following error.
    Argum — missed 185, right 24, extra 17
- [ ] **TS7027** Unreachable code detected. — missed 162, right 185, extra 19
- [ ] **TS2353** Object literal may only specify known properties, and 'X' does not exist in type 'X'. — missed 148, right 4, extra 15
- [ ] **TS7053** Element implicitly has an 'X' type because expression of type 'X' can'X'{}'X'string'X'{}'. — missed 145, right 40, extra 86
- [ ] **TS2430** Interface 'X' incorrectly extends interface 'X'.
  Types of property 'X' are incompatible. — missed 143, right 47, extra 25
- [ ] **TS2554** Expected N arguments, but got N. — missed 139, right 102, extra 89
- [ ] **TS2411** Property 'X' of type 'X' is not assignable to 'X' index type 'X'. — missed 136, right 90, extra 11
- [ ] **TS7010** 'X', which lacks return-type annotation, implicitly has an 'X' return type. — missed 131, right 2,224
- [ ] **TS2664** Invalid module name in augmentation, module 'X' cannot be found. — missed 126, right 2
- [ ] **TS7020** Call signature, which lacks return-type annotation, implicitly has an 'X' return type. — missed 90, right 78
- [ ] **TS2416** Property 'X' in type 'X' is not assignable to the same property in base type 'X'.
  Type ' — missed 88, right 26, extra 19
- [ ] **TS7032** Property 'X' implicitly has type 'X', because its set accessor lacks a parameter type anno — missed 81, right 44
- [ ] **TS2540** Cannot assign to 'X' because it is a read-only property. — missed 79, right 18, extra 3
- [ ] **TS7013** Construct signature, which lacks return-type annotation, implicitly has an 'X' return type — missed 78, right 82
- [ ] **TS2741** Property 'X' is missing in type 'X' but required in type 'X'. — missed 78, right 73, extra 76
- [ ] **TS2693** 'X' only refers to a type, but is being used as a value here. — missed 68, right 16, extra 241
- [ ] **TS1183** An implementation cannot be declared in ambient contexts. — missed 65, right 21
- [ ] **TS2415** Class 'X' incorrectly extends base class 'X'.
  Property 'X' is private in type 'X' but no — missed 56, right 0, extra 13
- [ ] **TS2488** Type 'X' must have a 'X' method that returns an iterator. — missed 46, right 36, extra 57
- [ ] **TS2374** Duplicate index signature for type 'X'. — missed 45, right 18
- [ ] **TS2393** Duplicate function implementation. — missed 41, right 10
- [ ] **TS2344** Type 'X' does not satisfy the constraint 'X'. — missed 39, right 59, extra 144
- [ ] **TS2349** This expression is not callable.
  Type 'X' has no call signatures. — missed 34, right 17, extra 143
- [ ] **TS2341** Property 'X' is private and only accessible within class 'X'. — missed 34, right 77, extra 20
- [ ] **TS2551** Property 'X' does not exist on type 'X'. Did you mean 'X'? — missed 31, right 14, extra 35
- [ ] **TS2428** All declarations of 'X' must have identical type parameters. — missed 30, right 30, extra 6
- [ ] **TS2351** This expression is not constructable.
  Type 'X' has no construct signatures. — missed 27, right 6, extra 13
- [ ] **TS2420** Class 'X' incorrectly implements interface 'X'.
  Property 'X' is missing in type 'X' but  — missed 24, right 36, extra 47
- [ ] **TS2493** Tuple type 'X' of length 'X' has no element at index 'X'. — missed 24, right 24, extra 1
- [ ] **TS7023** 'X' implicitly has return type 'X' because it does not have a return type annotation and i — missed 20, right 5, extra 3
- [ ] **TS2395** Individual declarations in merged declaration 'X' must be all exported or all local. — missed 20, right 18, extra 2
- [ ] **TS2355** A function whose declared type is neither 'X', 'X', nor 'X' must return a value. — missed 18, right 19, extra 1
- [ ] **TS1039** Initializers are not allowed in ambient contexts. — missed 18, right 16, extra 3
- [ ] **TS2740** Type 'X' is missing the following properties from type 'X': length, pop, push, concat, and — missed 18, right 0, extra 23
- [ ] **TS18047** 'X' is possibly 'X'. — missed 18, right 5, extra 26
- [ ] **TS2445** Property 'X' is protected and only accessible within class 'X' and its subclasses. — missed 17, right 84, extra 36
- [ ] **TS2558** Expected N type arguments, but got N. — missed 16, right 51, extra 5
- [ ] **TS2314** Generic type 'X' requires N type argument(s). — missed 14, right 127, extra 30
- [ ] **TS2531** Object is possibly 'X'. — missed 14, right 0, extra 231
- [ ] **TS6053** File 'X' not found. — missed 14, right 18, extra 1
- [ ] **TS2739** Type 'X' is missing the following properties from type 'X': hello, world — missed 13, right 18, extra 50
- [ ] **TS2717** Subsequent property declarations must have the same type.  Property 'X' must be of type 'X — missed 12, right 3, extra 3
- [ ] **TS2366** Function lacks ending return statement and return type does not include 'X'. — missed 10, right 17, extra 20
- [ ] **TS7030** Not all code paths return a value. — missed 8, right 58, extra 33
- [ ] **TS2567** Enum declarations can only merge with namespace or other enum declarations. — missed 7, right 26
- [ ] **TS7029** Fallthrough case in switch. — missed 7, right 36, extra 2
- [ ] **TS2556** A spread argument must either have a tuple type or be passed to a rest parameter. — missed 7, right 12
- [ ] **TS2315** Type 'X' is not generic. — missed 6, right 9
- [ ] **TS2590** Expression produces a union type that is too complex to represent. — missed 5, right 1, extra 1
- [ ] **TS2555** Expected at least N arguments, but got N. — missed 5, right 5
- [ ] **TS2588** Cannot assign to 'X' because it is a constant. — missed 4, right 17, extra 13
- [ ] **TS2451** Cannot redeclare block-scoped variable 'X'. — missed 4, right 53, extra 9
- [ ] **TS2669** Augmentations for the global scope can only be directly nested in external modules or ambi — missed 4, right 0, extra 4
- [ ] **TS1265** A rest element cannot follow another rest element. — missed 3, right 2
- [ ] **TS2514** A tuple type cannot be indexed with a negative value. — missed 1, right 1

## 2. Codes tsox never reports (new checks)

- [ ] **TS18050** The value 'X' cannot be used here. — missed 722
- [ ] **TS1005** 'X' expected. — missed 523
- [ ] **TS2367** This comparison appears to be unintentional because the types 'X' and 'X' have no overlap. — missed 384
- [ ] **TS7019** Rest parameter 'X' implicitly has an 'X' type. — missed 303
- [ ] **TS7005** Variable 'X' implicitly has an 'X' type. — missed 290
- [ ] **TS2550** Property 'X' does not exist on type 'X'. Do you need to change your target library? Try ch — missed 267
- [ ] **TS1109** Expression expected. — missed 246
- [ ] **TS2695** Left side of comma operator is unused and has no side effects. — missed 225
- [ ] **TS2356** An arithmetic operand must be of type 'X', 'X', 'X' or an enum type. — missed 203
- [ ] **TS2403** Subsequent variable declarations must have the same type.  Variable 'X' must be of type 'X — missed 201
- [ ] **TS7031** Binding element 'X' implicitly has an 'X' type. — missed 189
- [ ] **TS2352** Conversion of type 'X' to type 'X' may be a mistake because neither type sufficiently over — missed 160
- [ ] **TS1214** Identifier expected. 'X' is a reserved word in strict mode. Modules are automatically in s — missed 155
- [ ] **TS1128** Declaration or statement expected. — missed 138
- [ ] **TS1215** Invalid use of 'X'. Modules are automatically in strict mode. — missed 136
- [ ] **TS17006** An unary expression with the 'X' operator is not allowed in the left-hand side of an expon — missed 132
- [ ] **TS2591** Cannot find name 'X'. Do you need to install type definitions for node? Try `npm i --save- — missed 127
- [ ] **TS1240** Unable to resolve signature of property decorator when called as an expression.
  Argument — missed 127
- [ ] **TS2872** This kind of expression is always truthy. — missed 120
- [ ] **TS7022** 'X' implicitly has type 'X' because it does not have a type annotation and is referenced d — missed 119
- [ ] **TS7009** 'X' expression, whose target lacks a construct signature, implicitly has an 'X' type. — missed 116
- [ ] **TS2503** Cannot find namespace 'X'. — missed 115
- [ ] **TS2703** The operand of a 'X' operator must be a property reference. — missed 110
- [ ] **TS4111** Property 'X' comes from an index signature, so it must be accessed with ['X']. — missed 109
- [ ] **TS2449** Class 'X' used before its declaration. — missed 105
- [ ] **TS2873** This kind of expression is always falsy. — missed 103
- [ ] **TS2694** Namespace 'X' has no exported member 'X'. — missed 94
- [ ] **TS1487** Octal escape sequences are not allowed. Use the syntax 'X'. — missed 88
- [ ] **TS2369** A parameter property is only allowed in a constructor implementation. — missed 81
- [ ] **TS2391** Function implementation is missing or not immediately following the declaration. — missed 77
- [ ] **TS1121** Octal literals are not allowed. Use the syntax 'X'. — missed 76
- [ ] **TS2464** A computed property name must be of type 'X', 'X', 'X', or 'X'. — missed 75
- [ ] **TS1344** 'A label is not allowed here. — missed 74
- [ ] **TS2364** The left-hand side of an assignment expression must be a variable or a property access. — missed 74
- [ ] **TS1206** Decorators are not valid here. — missed 69
- [ ] **TS2538** Type 'X' cannot be used as an index type. — missed 66
- [ ] **TS2357** The operand of an increment or decrement operator must be a variable or a property access. — missed 65
- [ ] **TS1166** A computed property name in a class property declaration must have a simple literal type o — missed 63
- [ ] **TS2699** Static property 'X' conflicts with built-in property 'X' of constructor function 'X'. — missed 63
- [ ] **TS17009** 'X' must be called before accessing 'X' in the constructor of a derived class. — missed 61
- [ ] **TS1161** Unterminated regular expression literal. — missed 61
- [ ] **TS1210** Code contained in a class is evaluated in JavaScript'X'arguments'. For more information, s — missed 57
- [ ] **TS2448** Block-scoped variable 'X' used before its declaration. — missed 57
- [ ] **TS2678** Type 'X' is not comparable to type 'X'. — missed 54
- [ ] **TS1029** 'X' modifier must precede 'X' modifier. — missed 54
- [ ] **TS2774** This condition will always return true since this function is always defined. Did you mean — missed 54
- [ ] **TS2394** This overload signature is not compatible with its implementation signature. — missed 53
- [ ] **TS1117** An object literal cannot have multiple properties with the same name. — missed 53
- [ ] **TS7034** Variable 'X' implicitly has type 'X' in some locations where its type cannot be determined — missed 52
- [ ] **TS1125** Hexadecimal digit expected. — missed 50
- [ ] **TS2842** 'X' is an unused renaming of 'X'. Did you intend to use it as a type annotation? — missed 49
- [ ] **TS1241** Unable to resolve signature of method decorator when called as an expression.
  The runtim — missed 49
- [ ] **TS1335** 'X' types are not allowed here. — missed 49
- [ ] **TS2537** Type 'X' has no matching index signature for type 'X'. — missed 48
- [ ] **TS2749** 'X' refers to a value, but is being used as a type here. Did you mean 'X'? — missed 47
- [ ] **TS1270** Decorator function return type 'X' is not assignable to type 'X'. — missed 47
- [ ] **TS2729** Property 'X' is used before its initialization. — missed 46
- [ ] **TS1003** Identifier expected. — missed 44
- [ ] **TS1102** 'X' cannot be called on an identifier in strict mode. — missed 44
- [ ] **TS1127** Invalid character. — missed 43

Long tail: 556 more codes, 5,216 missed lines in total (per-part results: `tsox_diagnostics = 0`).

TS1005, TS1109 and the other TS1xxx codes are parser diagnostics, so they belong with the parser, not the checker.

## 3. False positives (tsox reports, tsc does not)

- [ ] **TS2339** Property 'X' does not exist on type 'X'. — extra 1,244, right 429
- [ ] **TS2322** Type 'X' is not assignable to type 'X'. — extra 951, right 1,586
- [ ] **TS2345** Argument of type 'X' is not assignable to parameter of type 'X'. — extra 621, right 333
- [ ] **TS18048** 'X' is possibly 'X'. — extra 271, right 72
- [ ] **TS2693** 'X' only refers to a type, but is being used as a value here. — extra 241, right 16
- [ ] **TS2531** Object is possibly 'X'. — extra 231, right 0
- [ ] **TS2454** Variable 'X' is used before being assigned. — extra 144, right 3,052
- [ ] **TS2344** Type 'X' does not satisfy the constraint 'X'. — extra 144, right 59
- [ ] **TS2349** This expression is not callable.
  Type 'X' has no call signatures. — extra 143, right 17
- [ ] **TS7006** Parameter 'X' implicitly has an 'X' type. — extra 121, right 1,216
- [ ] **TS2304** Cannot find name 'X'. — extra 98, right 942
- [ ] **TS2554** Expected N arguments, but got N. — extra 89, right 102
- [ ] **TS7053** Element implicitly has an 'X' type because expression of type 'X' can'X'{}'X'string'X'{}'. — extra 86, right 40
- [ ] **TS2741** Property 'X' is missing in type 'X' but required in type 'X'. — extra 76, right 73
- [ ] **TS2488** Type 'X' must have a 'X' method that returns an iterator. — extra 57, right 36
- [ ] **TS4114** This member must have an 'X' modifier because it overrides a member in the base class 'X'. — extra 51, right 288
- [ ] **TS2739** Type 'X' is missing the following properties from type 'X': hello, world — extra 50, right 18
- [ ] **TS2420** Class 'X' incorrectly implements interface 'X'.
  Property 'X' is missing in type 'X' but  — extra 47, right 36
- [ ] **TS2445** Property 'X' is protected and only accessible within class 'X' and its subclasses. — extra 36, right 84
- [ ] **TS2551** Property 'X' does not exist on type 'X'. Did you mean 'X'? — extra 35, right 14
- [ ] **TS2434** A namespace declaration cannot be located prior to a class or function with which it is me — extra 34, right 11
- [ ] **TS7008** Member 'X' implicitly has an 'X' type. — extra 33, right 637
- [ ] **TS7030** Not all code paths return a value. — extra 33, right 58
- [ ] **TS2314** Generic type 'X' requires N type argument(s). — extra 30, right 127
- [ ] **TS2365** Operator 'X' cannot be applied to types 'X' and 'X'. — extra 28, right 65

## 4. Files that hang or crawl

The first two are left out of the runs above; the third is in them.

- [ ] `part-07/compiler__largeTupleTypes.ts`: does not finish in 8 s alone.
- [ ] `part-08/compiler__performanceComparisonOfStructurallyIdenticalInterfacesWithGenericSignatures.ts`: 25 ms on `b0efa6a`, over 30 s since `bfd1712` (the interface heritage check). Cost grows about n! with the number of generic methods that return their own interface (9 methods 36 s). The pairs compared repeat (56 distinct for the corpus file), but the binder environment differs on every route, so a cache keyed on the binder list never hits; keyed on the binders as a set it takes about 220 ms and stays exponential in the method count. Not retested at `fb9ebd6`.
- [ ] `part-09/compiler__relationComplexityError.ts`: about 5 s alone.
- [ ] The same corpus file as above reports TS2416 on line 45 ("'await' in type 'A' is not assignable ... base type 'InterfaceB<error>'"); tsc reports nothing. It also did so before `bfd1712`.

## 5. Already moved by the roadmap work

Counts are from this run (per part). Commits are the ones whose subject names the code.

- **TS2454** (37530b7 Report variables read before assignment): tsc 5,798 lines, right 3,052, missed 1,241, extra 144
- **TS7030** (02afd9d Report functions that can run off their end): tsc 66 lines, right 58, missed 8, extra 33
- **TS2366** (02afd9d): tsc 27 lines, right 17, missed 10, extra 20
- **TS2355** (02afd9d): tsc 37 lines, right 19, missed 18, extra 1
- **TS2534** (02afd9d): tsc 2 lines, right 2, missed 0, extra 2
- **TS2540** (118447a Model readonly properties and report writes to them): tsc 97 lines, right 18, missed 79, extra 3
- **TS2551** (fb9ebd6 Type library calls more precisely and suggest spellings): tsc 45 lines, right 14, missed 31, extra 35

## Notes

- tsgo is not a valid reference for the whole corpus: like the tsc CLI, it reports only syntax errors and type-checks nothing while any file in the project has one, and many corpus files have them on purpose. On `tests/fixtures` it matches tsc on every error line but words 8 files differently (message or code), so check those against tsc.
- Timings from the single-core sandbox are not representative: the commits that load files in parallel cannot help there.

## Refreshing

Run `scripts/ts-diag-tool/compare.js <part dir> <tsox> --module --json --runs 1 --no-cache` on each `part-NN` folder with tsc as the reference (not `--tsgo`), then group the lines by code across parts. Do not run the whole corpus as one project for per-code numbers.
