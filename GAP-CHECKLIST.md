# Gap checklist against tsc

Where tsox disagrees with tsc 6.0.3 on the 10k conformance corpus, by error code, so the work can be ordered by how many corpus lines it affects. Tick an item when its corpus count has gone down in a fresh run. Section 3 is the false-positive list (the main goal now); section 5 lists what the roadmap work has already moved; sections 6 and 7 are performance and real-project findings.

- Measured: tsox `196670a` (release), tsc 6.0.3 through the API, **each of the 21 parts as its own project**, 10,081 files. Two files are left out because tsox does not finish on them (first two in section 4).
- Unit: a *line* is one file and line that has the code in tsc's output. *Missed* = tsc reports the code there and tsox does not. *Extra* = tsox reports it and tsc does not. *Right* = both report it.
- Files: exact match 4,317 (42.8%), of which 3,106 are clean on both sides; gap 4,140 (41.1%); false positive 658 (6.5%); mixed 354; code differs 437; message differs 175. Files agreeing on error lines: 4,929 (48.9%). Exact lines: 12,009 of 38,702 (31.0%).
- Diagnostics: tsc 45,542, tsox 22,910.
- False positives, per-part runs: files with only false positives 685 (`fb9ebd6`), 666 (`2badb7c`), 658 (`196670a`); including mixed 1,058, 1,026, 1,012; extra lines (all codes) 5,254, 5,155, 5,116.
- Totals: 25,229 missed line-codes; 11,987 (48%) from 611 codes tsox never reports, 13,242 (52%) from 77 codes it reports only partly.

## Method: run the corpus part by part

Running the whole corpus as ONE project gives a wrong reference for some codes: tsc reported 322 TS2454 lines that way, and 5,798 when each part is its own project (most of the 3,081 false-positive TS2454 lines in the single run were really agreement). Counts from a one-project run are not comparable with these. Why one project loses diagnostics is not understood; the corpus README already says to run one folder at a time.

## 1. Codes tsox reports only partly (completeness)

Highest yield: the check exists, so this is breadth. Sorted by missed lines.

- [ ] **TS2304** Cannot find name 'X'. — missed 1,843, right 927, extra 73
- [ ] **TS2322** Type 'X' is not assignable to type 'X'. — missed 1,460, right 1,587, extra 943
- [ ] **TS2454** Variable 'X' is used before being assigned. — missed 1,241, right 3,052, extra 144
- [ ] **TS7006** Parameter 'X' implicitly has an 'X' type. — missed 718, right 1,216, extra 121
- [ ] **TS2339** Property 'X' does not exist on type 'X'. — missed 611, right 429, extra 1,223
- [ ] **TS2362** The left-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum typ — missed 495, right 430, extra 17
- [ ] **TS2345** Argument of type 'X' is not assignable to parameter of type 'X'. — missed 486, right 332, extra 587
- [ ] **TS2564** Property 'X' has no initializer and is not definitely assigned in the constructor. — missed 463, right 1,397, extra 8
- [ ] **TS2300** Duplicate identifier 'X'. — missed 445, right 122, extra 7
- [ ] **TS2363** The right-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum ty — missed 386, right 393, extra 28
- [ ] **TS2365** Operator 'X' cannot be applied to types 'X' and 'X'. — missed 363, right 65, extra 28
- [ ] **TS4114** This member must have an 'X' modifier because it overrides a member in the base class 'X'. — missed 346, right 290, extra 17
- [ ] **TS2367** This comparison appears to be unintentional because the types 'X' and 'X' have no overlap. — missed 322, right 62, extra 7
- [ ] **TS18048** 'X' is possibly 'X'. — missed 284, right 72, extra 268
- [ ] **TS2532** Object is possibly 'X'. — missed 253, right 38, extra 7
- [ ] **TS2307** Cannot find module 'X' or its corresponding type declarations. — missed 227, right 105, extra 6
- [ ] **TS7008** Member 'X' implicitly has an 'X' type. — missed 205, right 635, extra 23
- [ ] **TS2683** 'X' implicitly has type 'X' because it does not have a type annotation. — missed 203, right 35
- [ ] **TS2769** No overload matches this call. Overload N of N, 'X', gave the following error. Argum — missed 180, right 29, extra 17
- [ ] **TS7027** Unreachable code detected. — missed 162, right 185, extra 19
- [ ] **TS2352** Conversion of type 'X' to type 'X' may be a mistake because neither type sufficiently over — missed 158, right 2
- [ ] **TS2353** Object literal may only specify known properties, and 'X' does not exist in type 'X'. — missed 148, right 4, extra 15
- [ ] **TS7053** Element implicitly has an 'any' type because the expression can't be used to index the type. — missed 145, right 40, extra 83
- [ ] **TS2430** Interface 'X' incorrectly extends interface 'X'. Types of property 'X' are incompatible. — missed 143, right 47, extra 25
- [ ] **TS2554** Expected N arguments, but got N. — missed 139, right 102, extra 85
- [ ] **TS2411** Property 'X' of type 'X' is not assignable to 'X' index type 'X'. — missed 136, right 90, extra 11
- [ ] **TS7010** 'X', which lacks return-type annotation, implicitly has an 'X' return type. — missed 131, right 2,224
- [ ] **TS2664** Invalid module name in augmentation, module 'X' cannot be found. — missed 126, right 2
- [ ] **TS7020** Call signature, which lacks return-type annotation, implicitly has an 'X' return type. — missed 90, right 78
- [ ] **TS2416** Property 'X' in type 'X' is not assignable to the same property in base type 'X'. Type ' — missed 86, right 28, extra 20
- [ ] **TS7032** Property 'X' implicitly has type 'X', because its set accessor lacks a parameter type anno — missed 81, right 44
- [ ] **TS7013** Construct signature, which lacks return-type annotation, implicitly has an 'X' return type — missed 78, right 82
- [ ] **TS2741** Property 'X' is missing in type 'X' but required in type 'X'. — missed 78, right 73, extra 76
- [ ] **TS2540** Cannot assign to 'X' because it is a read-only property. — missed 76, right 21, extra 3
- [ ] **TS2693** 'X' only refers to a type, but is being used as a value here. — missed 68, right 16, extra 241
- [ ] **TS1183** An implementation cannot be declared in ambient contexts. — missed 65, right 21
- [ ] **TS2415** Class 'X' incorrectly extends base class 'X'. Property 'X' is private in type 'X' but no — missed 56, right 0, extra 12
- [ ] **TS4111** Property 'X' comes from an index signature, so it must be accessed with ['X']. — missed 53, right 56, extra 5
- [ ] **TS2488** Type 'X' must have a 'X' method that returns an iterator. — missed 46, right 36, extra 55
- [ ] **TS2374** Duplicate index signature for type 'X'. — missed 45, right 18
- [ ] **TS2393** Duplicate function implementation. — missed 41, right 10
- [ ] **TS2344** Type 'X' does not satisfy the constraint 'X'. — missed 38, right 60, extra 144
- [ ] **TS2349** This expression is not callable. Type 'X' has no call signatures. — missed 34, right 17, extra 143
- [ ] **TS2341** Property 'X' is private and only accessible within class 'X'. — missed 34, right 77, extra 20
- [ ] **TS2551** Property 'X' does not exist on type 'X'. Did you mean 'X'? — missed 31, right 14, extra 33
- [ ] **TS2428** All declarations of 'X' must have identical type parameters. — missed 30, right 30, extra 6
- [ ] **TS2351** This expression is not constructable. Type 'X' has no construct signatures. — missed 27, right 6, extra 13
- [ ] **TS2542** Index signature in type 'X' only permits reading. — missed 27, right 1
- [ ] **TS2420** Class 'X' incorrectly implements interface 'X'. Property 'X' is missing in type 'X' but  — missed 24, right 36, extra 44
- [ ] **TS2493** Tuple type 'X' of length 'X' has no element at index 'X'. — missed 22, right 26, extra 1
- [ ] **TS7023** 'X' implicitly has return type 'X' because it does not have a return type annotation and i — missed 20, right 5, extra 3
- [ ] **TS2395** Individual declarations in merged declaration 'X' must be all exported or all local. — missed 20, right 18, extra 2
- [ ] **TS2355** A function whose declared type is neither 'X', 'X', nor 'X' must return a value. — missed 18, right 19, extra 1
- [ ] **TS1039** Initializers are not allowed in ambient contexts. — missed 18, right 16, extra 3
- [ ] **TS2740** Type 'X' is missing the following properties from type 'X': length, pop, push, concat, and — missed 18, right 0, extra 23
- [ ] **TS18047** 'X' is possibly 'X'. — missed 18, right 5, extra 25
- [ ] **TS2558** Expected N type arguments, but got N. — missed 17, right 50, extra 5
- [ ] **TS2445** Property 'X' is protected and only accessible within class 'X' and its subclasses. — missed 17, right 84, extra 36
- [ ] **TS2314** Generic type 'X' requires N type argument(s). — missed 14, right 127, extra 30
- [ ] **TS2531** Object is possibly 'X'. — missed 14, right 0, extra 231
- [ ] **TS6053** File 'X' not found. — missed 14, right 18, extra 1
- [ ] **TS2739** Type 'X' is missing the following properties from type 'X': hello, world — missed 13, right 18, extra 50
- [ ] **TS2717** Subsequent property declarations must have the same type.  Property 'X' must be of type 'X — missed 12, right 3, extra 3
- [ ] **TS2366** Function lacks ending return statement and return type does not include 'X'. — missed 10, right 17, extra 20
- [ ] **TS4104** The type 'X' is 'X' and cannot be assigned to the mutable type 'X'. — missed 9, right 1
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
- [ ] **TS7019** Rest parameter 'X' implicitly has an 'X' type. — missed 303
- [ ] **TS7005** Variable 'X' implicitly has an 'X' type. — missed 290
- [ ] **TS2550** Property 'X' does not exist on type 'X'. Do you need to change your target library? Try ch — missed 267
- [ ] **TS1109** Expression expected. — missed 246
- [ ] **TS2695** Left side of comma operator is unused and has no side effects. — missed 225
- [ ] **TS2356** An arithmetic operand must be of type 'X', 'X', 'X' or an enum type. — missed 203
- [ ] **TS2403** Subsequent variable declarations must have the same type.  Variable 'X' must be of type 'X — missed 201
- [ ] **TS7031** Binding element 'X' implicitly has an 'X' type. — missed 189
- [ ] **TS1214** Identifier expected. 'X' is a reserved word in strict mode. Modules are automatically in s — missed 155
- [ ] **TS1128** Declaration or statement expected. — missed 138
- [ ] **TS1215** Invalid use of 'X'. Modules are automatically in strict mode. — missed 136
- [ ] **TS17006** An unary expression with the 'X' operator is not allowed in the left-hand side of an expon — missed 132
- [ ] **TS2591** Cannot find name 'X'. Do you need to install type definitions for node? Try `npm i --save- — missed 127
- [ ] **TS1240** Unable to resolve signature of property decorator when called as an expression. Argument — missed 127
- [ ] **TS2872** This kind of expression is always truthy. — missed 120
- [ ] **TS7022** 'X' implicitly has type 'X' because it does not have a type annotation and is referenced d — missed 119
- [ ] **TS7009** 'X' expression, whose target lacks a construct signature, implicitly has an 'X' type. — missed 116
- [ ] **TS2503** Cannot find namespace 'X'. — missed 115
- [ ] **TS2703** The operand of a 'X' operator must be a property reference. — missed 110
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
- [ ] **TS1241** Unable to resolve signature of method decorator when called as an expression. The runtim — missed 49
- [ ] **TS1335** 'X' types are not allowed here. — missed 49
- [ ] **TS2537** Type 'X' has no matching index signature for type 'X'. — missed 48
- [ ] **TS2749** 'X' refers to a value, but is being used as a type here. Did you mean 'X'? — missed 47
- [ ] **TS1270** Decorator function return type 'X' is not assignable to type 'X'. — missed 47
- [ ] **TS2729** Property 'X' is used before its initialization. — missed 46
- [ ] **TS1003** Identifier expected. — missed 44
- [ ] **TS1102** 'X' cannot be called on an identifier in strict mode. — missed 44
- [ ] **TS1127** Invalid character. — missed 43
- [ ] **TS2790** The operand of a 'X' operator must be optional. — missed 41
- [ ] **TS1345** An expression of type 'X' cannot be tested for truthiness. — missed 40
- [ ] **TS2528** A module cannot have multiple default exports. — missed 39

Long tail: 551 more codes, 5,058 missed lines in total (per-part results: `tsox_diagnostics = 0`).

TS1005, TS1109 and the other TS1xxx codes are parser diagnostics, so they belong with the parser, not the checker.

## 3. False positives (tsox reports, tsc does not)

5,116 extra lines in total. The top 5 codes are 64% of them and the top 12 are 82%. Files and parts are of the diagnostics behind those lines (a few more than the line counts).

- [ ] **TS2339** Property 'X' does not exist on type 'X'. — extra 1,223, right 429 — in 315 files; most in part-12 213, part-16 195; top files `conformance__declarationEmit__nullPropertyName.t` 78, `conformance__jsdoc__declarations__jsDeclarations` 78
- [ ] **TS2322** Type 'X' is not assignable to type 'X'. — extra 943, right 1,587 — in 332 files; most in part-19 167, part-20 126; top files `conformance__types__typeRelationships__typeInfer` 30, `conformance__expressions__contextualTyping__gene` 24
- [ ] **TS2345** Argument of type 'X' is not assignable to parameter of type 'X'. — extra 587, right 332 — in 226 files; most in part-20 143, part-09 71; top files `conformance__types__tuple__variadicTuples1.ts` 34, `conformance__types__rest__genericRestParameters1` 23
- [ ] **TS18048** 'X' is possibly 'X'. — extra 268, right 72 — in 38 files; most in part-15 157, part-12 64; top files `conformance__controlFlow__controlFlowOptionalCha` 49, `conformance__expressions__binaryOperators__arith` 48
- [ ] **TS2693** 'X' only refers to a type, but is being used as a value here. — extra 241, right 16 — in 45 files; most in part-07 103, part-15 69; top files `compiler__longObjectInstantiationChain1.ts` 50, `compiler__longObjectInstantiationChain3.ts` 50
- [ ] **TS2531** Object is possibly 'X'. — extra 231, right 0 — in 23 files; most in part-15 220, part-16 18; top files `conformance__expressions__binaryOperators__compa` 64, `conformance__expressions__binaryOperators__arith` 48
- [ ] **TS2454** Variable 'X' is used before being assigned. — extra 144, right 3,052 — in 51 files; most in part-13 69, part-07 24; top files `conformance__statements__for-inStatements__for-i` 10, `compiler__narrowTypeByInstanceof.ts` 8
- [ ] **TS2344** Type 'X' does not satisfy the constraint 'X'. — extra 144, right 60 — in 81 files; most in part-09 37, part-19 32; top files `conformance__types__conditional__inferTypes1.ts` 15, `compiler__recursiveConditionalTypes.ts` 12
- [ ] **TS2349** This expression is not callable. Type 'X' has no call signatures. — extra 143, right 17 — in 56 files; most in part-21 35, part-10 22; top files `conformance__types__union__unionTypeReduction2.t` 15, `conformance__types__union__unionTypeCallSignatur` 12
- [ ] **TS7006** Parameter 'X' implicitly has an 'X' type. — extra 121, right 1,216 — in 41 files; most in part-02 76, part-08 20; top files `compiler__collisionArgumentsClassConstructor.ts` 10, `compiler__collisionArgumentsClassMethod.ts` 10
- [ ] **TS2554** Expected N arguments, but got N. — extra 85, right 102 — in 26 files; most in part-09 45, part-15 10; top files `compiler__promisePermutations2.ts` 45, `conformance__expressions__functionCalls__callWit` 8
- [ ] **TS7053** Element implicitly has an 'any' type because the expression can't be used to index the type. — extra 83, right 40 — in 42 files; most in part-08 25, part-02 8; top files `compiler__noImplicitAnyStringIndexerOnObject.ts` 19, `compiler__noImplicitAnyIndexing.ts` 3
- [ ] **TS2741** Property 'X' is missing in type 'X' but required in type 'X'. — extra 76, right 73
- [ ] **TS2304** Cannot find name 'X'. — extra 73, right 927
- [ ] **TS2488** Type 'X' must have a 'X' method that returns an iterator. — extra 55, right 36
- [ ] **TS2739** Type 'X' is missing the following properties from type 'X': hello, world — extra 50, right 18
- [ ] **TS2420** Class 'X' incorrectly implements interface 'X'. Property 'X' is missing in type 'X' but  — extra 44, right 36
- [ ] **TS2445** Property 'X' is protected and only accessible within class 'X' and its subclasses. — extra 36, right 84
- [ ] **TS2434** A namespace declaration cannot be located prior to a class or function with which it is me — extra 34, right 11
- [ ] **TS2551** Property 'X' does not exist on type 'X'. Did you mean 'X'? — extra 33, right 14
- [ ] **TS7030** Not all code paths return a value. — extra 33, right 58
- [ ] **TS2314** Generic type 'X' requires N type argument(s). — extra 30, right 127
- [ ] **TS2365** Operator 'X' cannot be applied to types 'X' and 'X'. — extra 28, right 65
- [ ] **TS2363** The right-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum ty — extra 28, right 393
- [ ] **TS2430** Interface 'X' incorrectly extends interface 'X'. Types of property 'X' are incompatible. — extra 25, right 47

Concentrated in few files (a handful of fixes clears many lines):
- [ ] **TS2531**: 249 diagnostics in 23 files; `conformance__expressions__binaryOperators__comparisonOp` 64, `conformance__expressions__binaryOperators__arithmeticOp` 48, `conformance__expressions__binaryOperators__arithmeticOp` 32
- [ ] **TS2693**: 248 diagnostics in 45 files; `compiler__longObjectInstantiationChain1.ts` 50, `compiler__longObjectInstantiationChain3.ts` 50, `conformance__expressions__functionCalls__typeArgumentIn` 39
- [ ] **TS18048**: 286 diagnostics in 38 files; `conformance__controlFlow__controlFlowOptionalChain.ts` 49, `conformance__expressions__binaryOperators__arithmeticOp` 48, `conformance__expressions__binaryOperators__arithmeticOp` 32

## 4. Files that hang or crawl

Last tested on `2badb7c`; not retested on `196670a`. The first two are left out of the runs above; the third is in them.

- [ ] `part-07/compiler__largeTupleTypes.ts`: does not finish in 15 s alone.
- [ ] `part-08/compiler__performanceComparisonOfStructurallyIdenticalInterfacesWithGenericSignatures.ts`: 25 ms on `b0efa6a`, over 30 s since `bfd1712` (the interface heritage check), still over 15 s on `2badb7c`. Cost grows about n! with the number of generic methods that return their own interface. The compared pairs repeat (56 distinct for this file), but the binder environment differs on every route. `337384b` memoizes pairs only when `binders` is empty, and these comparisons run inside generic method signatures, so the memo is skipped. A key over the binders *as a set* (order and repeats do not matter to the lookup) reuses the work; that took the file to about 220 ms in a draft and was still exponential in the method count (2^n sets).
- [ ] `part-09/compiler__relationComplexityError.ts`: regression, see section 6. 4 to 5 s before `337384b`, about 27 to 29 s after.
- [ ] The file in the second item also reports TS2416 on line 45 ("'await' in type 'A' is not assignable ... base type 'InterfaceB<error>'"); tsc reports nothing. It did so before `bfd1712` too.

## 5. Already moved by the roadmap work

Counts are from this run (per part). Commits are the ones whose subject names the code.

- **TS2454** (37530b7 Report variables read before assignment): tsc 5,798 lines, right 3,052, missed 1,241, extra 144
- **TS7030** (02afd9d Report functions that can run off their end): tsc 66 lines, right 58, missed 8, extra 33
- **TS2366** (02afd9d): tsc 27 lines, right 17, missed 10, extra 20
- **TS2355** (02afd9d): tsc 37 lines, right 19, missed 18, extra 1
- **TS2534** (02afd9d): tsc 2 lines, right 2, missed 0, extra 2
- **TS2540** (118447a Model readonly properties and report writes to them): tsc 97 lines, right 21, missed 76, extra 3
- **TS2551** (fb9ebd6 Type library calls more precisely and suggest spellings): tsc 45 lines, right 14, missed 31, extra 33
- **TS2367** (e8c5877 Report comparisons whose types cannot overlap; f43e9c9 and 337384b narrow the guard): tsc 385 lines, right 62, missed 322, extra 7
- **TS2352** (75a7f1a Report invalid type assertions for clearly non-overlapping types): tsc 160 lines, right 2, missed 158, extra 0
- **TS4111** (937e16a Report TS4111, type new Promise<T> and Promise.allSettled): tsc 141 lines, right 56, missed 53, extra 5
- **TS2542** (60d0000 Model readonly arrays): tsc 28 lines, right 1, missed 27, extra 0
- **TS4104** (60d0000 Model readonly arrays): tsc 10 lines, right 1, missed 9, extra 0

## 6. Performance regression

- [ ] `337384b` ("Memoize subtype pairs within a comparison"): `compiler__relationComplexityError.ts` takes 5.1 s on its parent `86d1abc` and 26.9 s on `337384b` (28.9 s on `2badb7c`), alone. Part-09 as a whole went from 18 s and 50 MB of tsox time and memory to 92 s and 663 MB (100 s and 664 MB on `196670a`); the other 20 parts are unchanged or slightly faster. The memo keeps every decided pair in a `HashMap` until the outermost comparison ends, so a file with very large unions probably grows it to millions of entries. Not profiled. Options: cap the memo, or only memoize comparisons that are large enough to repeat.

## 7. Real projects (tsox against tsgo, `2badb7c`, single core, 10 s limit)

Not rerun on `196670a`.

| project | tsgo | tsox | tsox, warm lib cache | errors tsox / tsgo |
|---|---|---|---|---|
| zustand | 1,061 ms, 195 MB | 819 ms, 48 MB | 263 ms | 40 / 0 |
| immer | 525 ms, 106 MB | 239 ms, 34 MB | 87 ms | 11 / 5 |
| jotai | 1,184 ms, 227 MB | 1,276 ms, 48 MB | 673 ms | 586 / 7 |
| rxjs | 8,281 ms, 1,167 MB | 2,570 ms, 59 MB | 1,947 ms | 3,751 / 22,014 (see below) |
| type-fest | over 10 s | over 10 s | over 10 s | not compared |

- [ ] **jotai, generic inference.** `store.get(atom)` with an intersection argument (`PrimitiveAtom<number> & WithInitialValue<number>`) and a parameter `Atom<Value>`: `Value` is never instantiated, so errors read `Atom<Value>` and `'Value' and '1'`. Probably one root cause behind most of its 586 errors: TS2345 329 extra lines, TS2339 67 (`.slice` on `Value`, callback parameters left `unknown`), TS2365 and TS2362 28 (arithmetic on `Value`). Inference should go through interface heritage (`PrimitiveAtom` to `WritableAtom` to `Atom`) and intersection members. `tests/vanilla/store.test.tsx` has 182 of the 586.
- [ ] **jotai, generic function type as an assignment target.** `() => void` is rejected for `OnMount<[SetStateAction<number>], void>` (TS2322, 63 lines); the callback parameters stay untyped.
- [ ] **jotai, constraint on a conditional type.** `ExtractAtomArgs<AtomType>` is reported as not satisfying `unknown[]` (TS2344, 13 lines).
- [ ] **jotai, narrowing.** `getDefault(get, options)` and `use(value)` reported "not callable" (TS2349, 25 lines); looks like a `typeof x === 'function'` narrowing of a union with a type parameter, or React's `use` typing. Not confirmed.
- [ ] **`process` and `globalThis` not found.** TS2304 on jotai (17 and 2), zustand (8). tsgo finds `process` through `@types/node`; automatic `@types` inclusion and `globalThis` are standard library and environment work.
- [ ] **immer, wrong code for a Node global.** tsox reports TS2304 (6 lines) where tsgo reports TS2591 (5 lines), "Cannot find name ... install type definitions for node". Probably the same names; not confirmed.
- [ ] **type-fest times out** at 10 s in both tsgo and tsox (almost pure type-level code).
- [ ] **rxjs needs a full install before it is a valid accuracy test.** With the dependencies installed (`--ignore-scripts`, `--legacy-peer-deps`, no workspace setup) tsgo itself reports 22,014 errors, mostly TS7031 and TS7006, so the 8,850 lines tsox is missing there are mostly implicit-any codes tsox does not report yet. Only its timing is usable: tsox is about 3.2x faster with 20x less memory.

## Notes

- tsgo is not a valid reference for the whole corpus: like the tsc CLI, it reports only syntax errors and type-checks nothing while any file in the project has one, and many corpus files have them on purpose. On `tests/fixtures` it matches tsc on every error line but words 8 files differently (message or code), so check those against tsc.
- Timings from the single-core sandbox are not representative: the commits that load files in parallel cannot help there.

## Refreshing

Run `scripts/ts-diag-tool/compare.js <part dir> <tsox> --module --json --runs 1 --no-cache` on each `part-NN` folder with tsc as the reference (not `--tsgo`), then group the lines by code across parts. Do not run the whole corpus as one project for per-code numbers.
