// Prints the type tsc gives each statement-level read of u, w or k (a statement that is only the
// name), one line per read: file:line:col type. Union members are sorted so that the order tsc
// prints them in does not matter.
const ts = require("../ts-diag-tool/node_modules/typescript");
const fs = require("fs"), path = require("path");
const dir = process.argv[2];
const files = fs.readdirSync(dir).filter((f) => f.endsWith(".ts")).map((f) => path.join(dir, f));
const program = ts.createProgram(files, { strict: true, noEmit: true, skipLibCheck: true,
  target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS });
const checker = program.getTypeChecker();
const norm = (t) => {
  const parts = t.isUnion() ? t.types.map((m) => checker.typeToString(m)) : [checker.typeToString(t)];
  return parts.sort().join(" | ");
};
for (const file of files) {
  const sf = program.getSourceFile(file);
  const visit = (node) => {
    if (ts.isExpressionStatement(node) && ts.isIdentifier(node.expression) &&
        ["u", "w", "k"].includes(node.expression.text)) {
      const id = node.expression;
      const { line, character } = sf.getLineAndCharacterOfPosition(id.getStart());
      console.log(`${file}:${line + 1}:${character + 1} ${norm(checker.getTypeAtLocation(id))}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
}
