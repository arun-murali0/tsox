#!/usr/bin/env node
// Records tsc's answers for a project part of the corpus (a folder of cases, each a folder of
// files): every case is compared on its own, as one project, with the real tsc, and the results
// are merged into one report in the shape compare.js --json writes, with each file named
// `case/relative/path`. `cargo xtask corpus run --real` calls it; the report goes to stdout.
//
// Usage: node corpus-cases.js <part-folder> <tsox-binary>

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const [part, tsoxBin] = process.argv.slice(2);
if (!part || !tsoxBin) {
  console.error("usage: node corpus-cases.js <part-folder> <tsox-binary>");
  process.exit(2);
}
const compare = path.join(__dirname, "compare.js");
const cases = fs
  .readdirSync(part, { withFileTypes: true })
  .filter((e) => e.isDirectory())
  .map((e) => e.name)
  .sort();

const files = [];
for (const name of cases) {
  const dir = path.join(part, name);
  const run = spawnSync(
    process.execPath,
    [compare, dir, tsoxBin, "--module", "--runs", "1", "--json"],
    { encoding: "utf8", maxBuffer: 1 << 28 },
  );
  let report;
  try {
    report = JSON.parse(run.stdout);
  } catch {
    process.stderr.write(`${name}: no report (status ${run.status})\n`);
    continue;
  }
  for (const f of report.files) {
    // compare.js names a file relative to the working directory; keep what is under the case.
    const abs = path.resolve(f.file);
    const rel = path.relative(path.resolve(part), abs).split(path.sep).join("/");
    files.push({ ...f, file: `${path.basename(path.resolve(part))}/${rel}` });
  }
}
process.stdout.write(JSON.stringify({ files }));
