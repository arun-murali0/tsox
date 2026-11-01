const ts = require('typescript');
const fs = require('node:fs');
const path = require('node:path');

const root = __dirname;
const expected = new Map([
  ['constraint_fallback.ts', []],
  ['independent_candidate_inference.ts', []],
  ['explicit_type_arguments.ts', []],
  ['multiple_argument_inference.ts', []],
  ['composite_parameter_inference.ts', []],
  ['contextual_callback_selection.ts', []],
  ['callback_return_drives_inference.ts', []],
  ['first_applicable_overload_wins.ts', []],
  ['broad_before_narrow.ts', []],
  ['arity_falls_through.ts', []],
  ['defaulted_generic_candidate.ts', []],
  ['no_overload_matches.ts', [2769]],
]);

const report = {
  compiler: `TypeScript ${ts.version}`,
  validation: 'oracle-only; this does not execute TSOX',
  fixtures: [],
};
let failed = false;
for (const [file, expectedCodes] of expected) {
  const fileName = path.join(root, file);
  const program = ts.createProgram([fileName], {
    noEmit: true,
    strict: true,
    skipLibCheck: true,
    target: ts.ScriptTarget.ES2020,
  });
  const diagnostics = ts.getPreEmitDiagnostics(program)
    .filter(d => d.file && path.resolve(d.file.fileName) === path.resolve(fileName))
    .map(d => ({
      code: d.code,
      message: ts.flattenDiagnosticMessageText(d.messageText, '\n'),
      line: d.file.getLineAndCharacterOfPosition(d.start ?? 0).line + 1,
    }));
  const actualCodes = diagnostics.map(d => d.code);
  const pass = JSON.stringify(actualCodes) === JSON.stringify(expectedCodes);
  failed ||= !pass;
  report.fixtures.push({ file, expectedCodes, actualCodes, pass, diagnostics });
}
report.passed = report.fixtures.filter(x => x.pass).length;
report.total = report.fixtures.length;
report.result = failed ? 'FAIL' : 'PASS';
fs.writeFileSync(path.join(root, 'oracle-report.json'), `${JSON.stringify(report, null, 2)}\n`);
console.log(`${report.result}: ${report.passed}/${report.total} fixtures matched ${report.compiler}`);
for (const fixture of report.fixtures) {
  console.log(`${fixture.pass ? 'PASS' : 'FAIL'} ${fixture.file}: ${fixture.actualCodes.join(', ') || 'no diagnostics'}`);
}
process.exitCode = failed ? 1 : 0;
