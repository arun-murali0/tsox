# Gap checklist against tsc

Where tsox disagrees with tsc 6.0.3 on the whole 10k conformance corpus, by error code, so the work can be ordered by how many corpus lines it affects. Tick an item when its corpus count has gone down in a fresh run.

- Measured: tsox `b6102db` (release), whole corpus as one project, 10,082 files (`part-07/compiler__largeTupleTypes.ts` left out: tsox does not finish on it).
- Unit: a *line* is one file and line that has the code in tsc's output. *Missed* = tsc reports the code there and tsox does not. *Extra* = tsox reports it and tsc does not.
- Files: exact match 4,247 (42.1%), of which 3,367 are clean on both sides; gap 3,900 (38.7%); false positive 1,031 (10.2%); mixed 466; code differs 318; message differs 120. Exact lines: 7,493 of 32,147 (23.3%).
- Totals: 21,114 missed line-codes; 13,431 (64%) from 614 codes tsox never reports, 7,683 (36%) from 48 codes it reports only partly.

## 1. Codes tsox reports only partly (completeness)

Highest yield: the check exists, so this is breadth. `right` = lines where both report it.

- [ ] **TS2304** Cannot find name 'X'. — missed 1,946, right 814, extra 18
- [ ] **TS7006** Parameter 'X' implicitly has an 'X' type. — missed 886, right 1,220, extra 116
- [ ] **TS2322** Type 'X' is not assignable to type 'X'. — missed 826, right 852, extra 1,662
- [ ] **TS2300** Duplicate identifier 'X'. — missed 434, right 122, extra 7
- [ ] **TS2564** Property 'X' has no initializer and is not definitely assigned in the constructor. — missed 411, right 1,330, extra 86
- [ ] **TS2339** Property 'X' does not exist on type 'X'. — missed 322, right 174, extra 2,124
- [ ] **TS2345** Argument of type 'X' is not assignable to parameter of type 'X'. — missed 265, right 227, extra 822
- [ ] **TS2307** Cannot find module 'X' or its corresponding type declarations. — missed 246, right 86, extra 6
- [ ] **TS2683** 'X' implicitly has type 'X' because it does not have a type annotation. — missed 204, right 35
- [ ] **TS7008** Member 'X' implicitly has an 'X' type. — missed 201, right 610, extra 59
- [ ] **TS2362** The left-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum typ — missed 182, right 150, extra 307
- [ ] **TS2353** Object literal may only specify known properties, and 'X' does not exist in type 'X'. — missed 142, right 3, extra 16
- [ ] **TS7027** Unreachable code detected. — missed 136, right 184, extra 19
- [ ] **TS7010** 'X', which lacks return-type annotation, implicitly has an 'X' return type. — missed 135, right 2,220
- [ ] **TS7053** Element implicitly has an 'X' type because expression of type 'X' can'X'typeof Base'. — missed 98, right 13, extra 116
- [ ] **TS18048** 'X' is possibly 'X'. — missed 97, right 3, extra 331
- [ ] **TS2363** The right-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum ty — missed 94, right 174, extra 257
- [ ] **TS7020** Call signature, which lacks return-type annotation, implicitly has an 'X' return type. — missed 90, right 78
- [ ] **TS7032** Property 'X' implicitly has type 'X', because its set accessor lacks a parameter type anno — missed 89, right 43
- [ ] **TS2488** Type 'X' must have a 'X' method that returns an iterator. — missed 89, right 34, extra 59
- [ ] **TS2532** Object is possibly 'X'. — missed 85, right 13, extra 26
- [ ] **TS2554** Expected N arguments, but got N. — missed 80, right 75, extra 326
- [ ] **TS7013** Construct signature, which lacks return-type annotation, implicitly has an 'X' return type — missed 78, right 82
- [ ] **TS2769** No overload matches this call. — missed 72, right 22, extra 42
- [ ] **TS2693** 'X' only refers to a type, but is being used as a value here. — missed 68, right 16, extra 235
- [ ] **TS2365** Operator 'X' cannot be applied to types 'X' and 'X'. — missed 64, right 17, extra 83
- [ ] **TS2741** Property 'X' is missing in type 'X' but required in type 'X'. — missed 46, right 41, extra 104
- [ ] **TS2556** A spread argument must either have a tuple type or be passed to a rest parameter. — missed 41, right 12
- [ ] **TS2344** Type 'X' does not satisfy the constraint 'X'. — missed 37, right 55, extra 146
- [ ] **TS2349** This expression is not callable. — missed 25, right 8, extra 139
- [ ] **TS2351** This expression is not constructable. — missed 22, right 1, extra 15
- [ ] **TS2314** Generic type 'X' requires N type argument(s). — missed 20, right 121, extra 22
- [ ] **TS2395** Individual declarations in merged declaration 'X' must be all exported or all local. — missed 20, right 18, extra 2
- [ ] **TS2493** Tuple type 'X' of length 'X' has no element at index 'X'. — missed 20, right 1, extra 23
- [ ] **TS6053** File 'X' not found. — missed 14, right 18, extra 1
- [ ] **TS2740** Type 'X' is missing the following properties from type 'X': length, pop, push, concat, and — missed 14, right 0, extra 101
- [ ] **TS2739** Type 'X' is missing the following properties from type 'X': a, b — missed 13, right 9, extra 43
- [ ] **TS2717** Subsequent property declarations must have the same type.  Property 'X' must be of type 'X — missed 12, right 3, extra 4
- [ ] **TS2531** Object is possibly 'X'. — missed 11, right 0, extra 233
- [ ] **TS2315** Type 'X' is not generic. — missed 8, right 7
- [ ] **TS7023** 'X' implicitly has return type 'X' because it does not have a return type annotation and i — missed 8, right 5, extra 3
- [ ] **TS7029** Fallthrough case in switch. — missed 7, right 37, extra 1
- [ ] **TS2567** Enum declarations can only merge with namespace or other enum declarations. — missed 7, right 26
- [ ] **TS2451** Cannot redeclare block-scoped variable 'X'. — missed 6, right 53, extra 9
- [ ] **TS2558** Expected N type arguments, but got N. — missed 4, right 40, extra 17
- [ ] **TS2590** Expression produces a union type that is too complex to represent. — missed 4, right 1, extra 1
- [ ] **TS1265** A rest element cannot follow another rest element. — missed 3, right 2
- [ ] **TS2514** A tuple type cannot be indexed with a negative value. — missed 1, right 0, extra 1

## 2. Codes tsox never reports (new checks)

- [ ] **TS18050** The value 'X' cannot be used here. — missed 684
- [ ] **TS4114** This member must have an 'X' modifier because it overrides a member in the base class 'X'. — missed 620
- [ ] **TS1005** 'X' expected. — missed 523
- [ ] **TS2454** Variable 'X' is used before being assigned. — missed 322 (started in `37530b7`: 148 of 322 lines now right on the corpus subset)
- [ ] **TS7019** Rest parameter 'X' implicitly has an 'X' type. — missed 306
- [ ] **TS7005** Variable 'X' implicitly has an 'X' type. — missed 260
- [ ] **TS1109** Expression expected. — missed 246
- [ ] **TS2695** Left side of comma operator is unused and has no side effects. — missed 225
- [ ] **TS2411** Property 'X' of type 'X' is not assignable to 'X' index type 'X'. — missed 222
- [ ] **TS2430** Interface 'X' incorrectly extends interface 'X'. — missed 191
- [ ] **TS7031** Binding element 'X' implicitly has an 'X' type. — missed 190
- [ ] **TS1214** Identifier expected. 'X' is a reserved word in strict mode. Modules are automatically in s — missed 155
- [ ] **TS2403** Subsequent variable declarations must have the same type.  Variable 'X' must be of type 'X — missed 153
- [ ] **TS2352** Conversion of type 'X' to type 'X' may be a mistake because neither type sufficiently over — missed 140
- [ ] **TS1128** Declaration or statement expected. — missed 138
- [ ] **TS1215** Invalid use of 'X'. Modules are automatically in strict mode. — missed 136
- [ ] **TS17006** An unary expression with the 'X' operator is not allowed in the left-hand side of an expon — missed 132
- [ ] **TS2664** Invalid module name in augmentation, module 'X' cannot be found. — missed 128
- [ ] **TS2591** Cannot find name 'X'. Do you need to install type definitions for node? Try `npm i --save- — missed 127
- [ ] **TS1240** Unable to resolve signature of property decorator when called as an expression. — missed 122
- [ ] **TS2872** This kind of expression is always truthy. — missed 120
- [ ] **TS2503** Cannot find namespace 'X'. — missed 114
- [ ] **TS2703** The operand of a 'X' operator must be a property reference. — missed 110
- [ ] **TS2416** Property 'X' in type 'X' is not assignable to the same property in base type 'X'. — missed 109
- [ ] **TS7022** 'X' implicitly has type 'X' because it does not have a type annotation and is referenced d — missed 105
- [ ] **TS2449** Class 'X' used before its declaration. — missed 104
- [ ] **TS2873** This kind of expression is always falsy. — missed 103
- [ ] **TS2367** This comparison appears to be unintentional because the types 'X' and 'X' have no overlap. — missed 97
- [ ] **TS2694** Namespace 'X' has no exported member 'X'. — missed 94
- [ ] **TS7009** 'X' expression, whose target lacks a construct signature, implicitly has an 'X' type. — missed 91
- [ ] **TS1487** Octal escape sequences are not allowed. Use the syntax 'X'. — missed 88
- [ ] **TS1183** An implementation cannot be declared in ambient contexts. — missed 86
- [ ] **TS2369** A parameter property is only allowed in a constructor implementation. — missed 81
- [ ] **TS2364** The left-hand side of an assignment expression must be a variable or a property access. — missed 80
- [ ] **TS2550** Property 'X' does not exist on type 'X'. Do you need to change your target library? Try ch — missed 80
- [ ] **TS1121** Octal literals are not allowed. Use the syntax 'X'. — missed 76
- [ ] **TS2391** Function implementation is missing or not immediately following the declaration. — missed 75
- [ ] **TS2356** An arithmetic operand must be of type 'X', 'X', 'X' or an enum type. — missed 75
- [ ] **TS1344** 'A label is not allowed here. — missed 74
- [ ] **TS2357** The operand of an increment or decrement operator must be a variable or a property access. — missed 71
- [ ] **TS2538** Type 'X' cannot be used as an index type. — missed 69
- [ ] **TS2341** Property 'X' is private and only accessible within class 'X'. — missed 69
- [ ] **TS1206** Decorators are not valid here. — missed 69
- [ ] **TS7030** Not all code paths return a value. — missed 68
- [ ] **TS2374** Duplicate index signature for type 'X'. — missed 63
- [ ] **TS1166** A computed property name in a class property declaration must have a simple literal type o — missed 63
- [ ] **TS2420** Class 'X' incorrectly implements interface 'X'. — missed 61
- [ ] **TS17009** 'X' must be called before accessing 'X' in the constructor of a derived class. — missed 61
- [ ] **TS1161** Unterminated regular expression literal. — missed 61
- [ ] **TS2540** Cannot assign to 'X' because it is a read-only property. — missed 60
- [ ] **TS2428** All declarations of 'X' must have identical type parameters. — missed 60
- [ ] **TS1210** Code contained in a class is evaluated in JavaScript'X'arguments'. For more information, s — missed 57
- [ ] **TS2448** Block-scoped variable 'X' used before its declaration. — missed 57
- [ ] **TS2415** Class 'X' incorrectly extends base class 'X'. — missed 56
- [ ] **TS1029** 'X' modifier must precede 'X' modifier. — missed 54
- [ ] **TS1125** Hexadecimal digit expected. — missed 50
- [ ] **TS2842** 'X' is an unused renaming of 'X'. Did you intend to use it as a type annotation? — missed 49
- [ ] **TS1335** 'X' types are not allowed here. — missed 49
- [ ] **TS2394** This overload signature is not compatible with its implementation signature. — missed 49
- [ ] **TS1241** Unable to resolve signature of method decorator when called as an expression. — missed 49

Long tail: 554 more codes, 5,404 missed lines in total (see `by_code.csv`, `tsox_diagnostics = 0`).

TS1005, TS1109 and the other TS1xxx codes are parser diagnostics, so they belong with the parser, not the checker.

## 3. False positives (tsox reports, tsc does not)

- [ ] **TS2339** Property 'X' does not exist on type 'X'. — extra 2,124, right 174
- [ ] **TS2322** Type 'X' is not assignable to type 'X'. — extra 1,662, right 852
- [ ] **TS2345** Argument of type 'X' is not assignable to parameter of type 'X'. — extra 822, right 227
- [ ] **TS18048** 'X' is possibly 'X'. — extra 331, right 3
- [ ] **TS2554** Expected N arguments, but got N. — extra 326, right 75
- [ ] **TS2362** The left-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum typ — extra 307, right 150
- [ ] **TS2363** The right-hand side of an arithmetic operation must be of type 'X', 'X', 'X' or an enum ty — extra 257, right 174
- [ ] **TS2693** 'X' only refers to a type, but is being used as a value here. — extra 235, right 16
- [ ] **TS2531** Object is possibly 'X'. — extra 233, right 0
- [ ] **TS2344** Type 'X' does not satisfy the constraint 'X'. — extra 146, right 55
- [ ] **TS2349** This expression is not callable. — extra 139, right 8
- [ ] **TS7006** Parameter 'X' implicitly has an 'X' type. — extra 116, right 1,220
- [ ] **TS7053** Element implicitly has an 'X' type because expression of type 'X' can'X'typeof Base'. — extra 116, right 13
- [ ] **TS2741** Property 'X' is missing in type 'X' but required in type 'X'. — extra 104, right 41
- [ ] **TS2740** Type 'X' is missing the following properties from type 'X': length, pop, push, concat, and — extra 101, right 0
- [ ] **TS2564** Property 'X' has no initializer and is not definitely assigned in the constructor. — extra 86, right 1,330
- [ ] **TS2365** Operator 'X' cannot be applied to types 'X' and 'X'. — extra 83, right 17
- [ ] **TS7008** Member 'X' implicitly has an 'X' type. — extra 59, right 610
- [ ] **TS2488** Type 'X' must have a 'X' method that returns an iterator. — extra 59, right 34
- [ ] **TS2739** Type 'X' is missing the following properties from type 'X': a, b — extra 43, right 9
- [ ] **TS2769** No overload matches this call. — extra 42, right 22
- [ ] **TS2434** A namespace declaration cannot be located prior to a class or function with which it is me — extra 34, right 11
- [ ] **TS18047**  — extra 28, right 0
- [ ] **TS2532** Object is possibly 'X'. — extra 26, right 13
- [ ] **TS2493** Tuple type 'X' of length 'X' has no element at index 'X'. — extra 23, right 1

## 4. Files that hang or crawl

- [ ] `part-07/compiler__largeTupleTypes.ts`: does not finish in 8 s alone; it stalled the whole-corpus run for over 7 minutes.
- [ ] `part-09/compiler__relationComplexityError.ts`: about 4 s alone.

## Refreshing

Run `scripts/compare-local.sh <corpus dir> --tsc --module --json` and group the diagnostics by code. tsc is the reference here: the tsgo CLI type-checks nothing while any file in the project has a syntax error, which this corpus has on purpose.
