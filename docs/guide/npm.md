# npm (WebAssembly)

```sh
npm install tsox
```

```js
const { TsoxChecker } = require("tsox");
const checker = new TsoxChecker();
console.log(checker.checkSource("const n: number = 'x';", "a.ts"));
```

In a browser or bundler, call `init()` first:

```js
import init, { TsoxChecker } from "tsox";
await init();
```

Each diagnostic has `severity`, `message`, `file_name` and one-based line and column
positions in UTF-16 units. The package checks one source string; it has no module resolution.
Use the command line or the language server for projects.
