// Checks the `// error CODE` markers in a fixture directory against real tsc (the oracle).
// Usage: node verify-markers.js <dir>   (strict, target ES2020, same as the lib-stress set)
// A fixture is only valid when every marked line has that code and no other line has any error.
const ts = require("typescript");
const fs = require("fs"), path = require("path");
const dir = process.argv[2];
const opts = { strict: true, noEmit: true, skipLibCheck: true, target: ts.ScriptTarget.ES2020, moduleDetection: ts.ModuleDetectionKind.Force };
let bad = 0, total = 0;
for (const f of fs.readdirSync(dir).filter(n => n.endsWith(".ts")).sort()) {
  const full = path.join(dir, f);
  const prog = ts.createProgram([full], opts);
  const sf = prog.getSourceFile(full);
  const diags = [...prog.getSyntacticDiagnostics(sf), ...prog.getSemanticDiagnostics(sf)];
  const got = new Map();
  for (const d of diags) { const l = sf.getLineAndCharacterOfPosition(d.start).line + 1; (got.get(l) || got.set(l, []).get(l)).push(d.code); }
  const want = new Map();
  fs.readFileSync(full, "utf8").split("\n").forEach((t, i) => { const m = t.match(/\/\/ error (\d+)/); if (m) want.set(i + 1, [Number(m[1])]); });
  const problems = [];
  for (const [l, c] of want) { const g = got.get(l); if (!g) problems.push(`line ${l}: expected ${c} but tsc reports nothing`); else if (!g.includes(c[0])) problems.push(`line ${l}: expected ${c} but tsc says ${g}`); }
  for (const [l, g] of got) if (!want.has(l)) problems.push(`line ${l}: tsc reports ${g} (unmarked)`);
  total++; if (problems.length) { bad++; console.log(`✗ ${f}`); problems.forEach(p => console.log("    " + p)); } else console.log(`✓ ${f}  (${want.size} errors)`);
}
console.log(`\n${total - bad}/${total} fixtures agree with tsc`);
