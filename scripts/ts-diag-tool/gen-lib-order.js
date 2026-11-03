// Prints the load order of a lib target: each file's `/// <reference lib="x" />` dependencies
// before it, in the order they appear, depth first (the order tsc processes them).
// Usage: node gen-lib-order.js <lib-dir> <root> [<root>...]    e.g. ... es2020.full
// Output: a JSON array of lib names (without the `lib.` prefix and `.d.ts` suffix).
const fs = require("fs"), path = require("path");
const [dir, ...roots] = process.argv.slice(2);
const seen = new Set(), out = [];
function visit(name) {
  if (seen.has(name)) return;
  seen.add(name);
  const text = fs.readFileSync(path.join(dir, `lib.${name}.d.ts`), "utf8");
  for (const m of text.matchAll(/\/\/\/\s*<reference\s+lib="([^"]+)"\s*\/>/g)) visit(m[1].toLowerCase());
  out.push(name);
}
roots.forEach(visit);
console.log(JSON.stringify(out));
