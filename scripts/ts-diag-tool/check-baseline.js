#!/usr/bin/env node
// Reads compare.js's --json output on stdin and baseline.json in this same
// directory, and decides whether a merge may go ahead.
//
// Problem: the old gate compared one number (the false-positive file count) to
// maxFalsePositive. Fixing one false positive while adding another left the number
// unchanged and passed, and a wrong code or message on a line both sides report
// (MISMATCH) was never looked at.
// Picked: baseline.json lists the known differences from tsc, one entry per
// (file, kind, tsc codes, tsox codes) with a count. Each run is compared to that
// list. Line numbers are left out of the key on purpose, so editing a fixture does
// not invalidate its entry.
//   - a FALSE POSITIVE or MISMATCH that is not listed fails the run
//   - a GAP that is not listed is only reported (a new fixture for a check that is
//     not built yet is not a regression); --strict makes it fail too
//   - a listed entry that no longer differs is reported as fixed; --strict fails on
//     it so the list only ever shrinks
// Cost: two differences in one file with the same kind and codes share one entry, so a
// swap of messages under identical codes is not noticed. Counts still catch an extra one.
//
// Gaps are the remaining work: the summary line prints how many are left, and
// baseline.json is the list to pick from.
//
// Usage (see .github/workflows/ci.yml, job compare-tsc, for the full pipeline):
//   node compare.js tests/fixtures target/release/tsox --json --differ --only-differ \
//     | node check-baseline.js [--strict]
//   ... | node check-baseline.js --update    # rewrite knownDifferences from this run
//
// While baseline.json has no knownDifferences yet, the old maxFalsePositive check is
// used, so this file can be merged first and the list generated afterwards with --update.

const fs = require("fs");
const path = require("path");

const args = process.argv.slice(2);
const strict = args.includes("--strict");
const update = args.includes("--update");

const baselinePath = path.join(__dirname, "baseline.json");
const shownPath = path.relative(process.cwd(), baselinePath);

const codesOf = (items) => items.map((text) => text.split(" ")[0]).join(",");
const keyOf = (d) =>
  [d.file, d.kind, codesOf(d.tsc), codesOf(d.tsox)].join("|");

let raw = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => (raw += chunk));
process.stdin.on("end", () => {
  let result;
  try {
    result = JSON.parse(raw);
  } catch (err) {
    console.error("error: could not parse compare.js --json output as JSON");
    console.error(err.message);
    process.exit(2);
  }

  const baseline = JSON.parse(fs.readFileSync(baselinePath, "utf8"));

  if (update) return writeBaseline(result, baseline);

  if (!Array.isArray(baseline.knownDifferences))
    return legacyCheck(result, baseline);

  if (!Array.isArray(result.differences)) {
    console.error(
      "error: no per-line differences in the input; run compare.js with --json --differ",
    );
    process.exit(2);
  }

  // actual: key -> { count, lines: [entries] }
  const actual = new Map();
  for (const d of result.differences) {
    const k = keyOf(d);
    if (!actual.has(k)) actual.set(k, { count: 0, entries: [] });
    const slot = actual.get(k);
    slot.count++;
    slot.entries.push(d);
  }
  const known = new Map();
  for (const e of baseline.knownDifferences) {
    known.set(
      [e.file, e.kind, e.tsc, e.tsox].join("|"),
      (known.get([e.file, e.kind, e.tsc, e.tsox].join("|")) || 0) +
        (e.count || 1),
    );
  }

  const regressions = []; // FALSE POSITIVE / MISMATCH not in the list
  const newGaps = [];
  for (const [k, slot] of actual) {
    if (slot.count <= (known.get(k) || 0)) continue;
    const bucket = slot.entries[0].kind === "GAP" ? newGaps : regressions;
    bucket.push(...slot.entries);
  }
  const fixed = [];
  for (const [k, count] of known) {
    if (count > (actual.has(k) ? actual.get(k).count : 0)) fixed.push(k);
  }

  const gapCount = result.differences.filter((d) => d.kind === "GAP").length;
  console.log(
    `differences from tsc: ${result.differences.length} line(s), ${gapCount} known-or-new gap(s) still to build`,
  );

  const show = (d) => {
    console.log(`  ${d.file}:${d.line}  ${d.kind}`);
    for (const t of d.tsc) console.log(`    tsc:     ${t.split("\n")[0]}`);
    for (const t of d.tsox) console.log(`    tsox: ${t.split("\n")[0]}`);
  };

  if (newGaps.length) {
    console.log(
      `\nnew gap(s) not in ${shownPath} (tsc reports, tsox does not):`,
    );
    newGaps.forEach(show);
  }
  if (fixed.length) {
    console.log(`\nlisted in ${shownPath} but no longer differ (fixed):`);
    for (const k of fixed)
      console.log(`  ${k.split("|").slice(0, 2).join("  ")}`);
  }
  if (newGaps.length || fixed.length)
    console.log(
      `\nRefresh the list with: ... | node ${path.relative(process.cwd(), __filename)} --update`,
    );

  if (regressions.length) {
    console.error(
      `\nFAIL: ${regressions.length} difference(s) from tsc that ${shownPath} does not list.`,
    );
    regressions.forEach(show);
    console.error(
      "\nA false positive means tsox rejects code tsc accepts; a mismatch means the " +
        "code or message is wrong. Fix the checker, or if the divergence is deliberate " +
        `(this project's own documented recovery behaviour), run --update and explain it in ${shownPath}.`,
    );
    process.exit(1);
  }
  if (strict && (newGaps.length || fixed.length)) {
    console.error(
      "\nFAIL (--strict): the known-differences list is out of date.",
    );
    process.exit(1);
  }
  process.exit(0);
});

function writeBaseline(result, baseline) {
  if (!Array.isArray(result.differences)) {
    console.error("error: --update needs compare.js --json --differ output");
    process.exit(2);
  }
  const counts = new Map();
  for (const d of result.differences) {
    const k = keyOf(d);
    counts.set(k, (counts.get(k) || 0) + 1);
  }
  const entries = [...counts]
    .map(([k, count]) => {
      const [file, kind, tsc, tsox] = k.split("|");
      return { file, kind, tsc, tsox, count };
    })
    .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));

  delete baseline.maxFalsePositive;
  // One entry per line keeps diffs small and lets two branches add entries without conflicts.
  const head = JSON.stringify({ ...baseline, knownDifferences: [] }, null, 2);
  const body = entries.map((e) => `    ${JSON.stringify(e)}`).join(",\n");
  const out = head.replace(
    /"knownDifferences": \[\]/,
    entries.length
      ? `"knownDifferences": [\n${body}\n  ]`
      : '"knownDifferences": []',
  );
  fs.writeFileSync(baselinePath, out + "\n");
  console.log(
    `wrote ${entries.length} entr${entries.length === 1 ? "y" : "ies"} ` +
      `(${result.differences.length} line(s)) to ${shownPath}`,
  );
}

function legacyCheck(result, baseline) {
  const actual = result.summary.falsePositive;
  const allowed = baseline.maxFalsePositive;
  console.log(`false positives: ${actual} (baseline allows up to ${allowed})`);
  console.log(
    `(baseline.json has no knownDifferences yet; generate it with --update to also gate mismatches)`,
  );
  if (actual > allowed) {
    console.error(
      `\nFAIL: false-positive count regressed from ${allowed} to ${actual}.\n` +
        `Run ./scripts/compare-local.sh --differ to see which fixture(s) newly regressed.`,
    );
    process.exit(1);
  }
  process.exit(0);
}
