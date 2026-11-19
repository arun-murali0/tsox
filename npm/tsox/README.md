# tsox

A TypeScript type checker written in Rust, built to WebAssembly. It checks one source string and
returns diagnostics. It is an early release: it reports what it finds itself and can differ from
`tsc`. Please report differences at https://github.com/tsoxide/tsox/issues.

## Node

```js
const { TsoxChecker } = require("tsox");
const checker = new TsoxChecker();
const diagnostics = checker.checkSource("const n: number = 'x';", "a.ts");
console.log(diagnostics);
```

## Browser / bundler

```js
import init, { TsoxChecker } from "tsox"; // or "tsox/web" to force the browser build
await init(); // fetches tsox_bg.wasm next to the module
const checker = new TsoxChecker();
console.log(checker.checkSource("let a: string = 1;", "a.ts"));
```

Each diagnostic has `severity`, `message`, `file_name` and one-based `start_line`,
`start_column`, `end_line`, `end_column` (columns count UTF-16 units, as JavaScript strings do).

This package checks a single file. It has no module resolution and no `lib` files beyond what is
built in. For projects use the `tsox` command line or `tsox-lsp`.
