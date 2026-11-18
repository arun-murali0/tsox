// Runs against a built package: `scripts/build-wasm.sh && node npm/tests/smoke.js`.
const assert = require("assert");
const { TsoxChecker } = require("../tsox");

const checker = new TsoxChecker();
const clean = checker.checkSource("export const n: number = 1;\n", "ok.ts");
assert.deepStrictEqual(clean, [], "a clean file has no diagnostics");

const bad = checker.checkSource("const n: number = 'x';\n", "bad.ts");
assert.ok(bad.length >= 1, "a type error is reported");
assert.strictEqual(bad[0].severity, "Error");
assert.strictEqual(bad[0].file_name, "bad.ts");
assert.strictEqual(bad[0].start_line, 1, "lines are one-based");
assert.strictEqual(bad[0].start_column, 19, "columns are one-based");

// UTF-16 columns: the astral character counts as two units.
const wide = checker.checkSource("const s = '😀'; const n: number = 'x';\n", "wide.ts");
assert.strictEqual(wide[0].start_column, 35, "the astral character counts as two UTF-16 units");
console.log("tsox wasm smoke test: ok");
