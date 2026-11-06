// tsgo (the native TypeScript 7 compiler) as the reference for compare.js --tsgo.
//
// Why: tsc through the JS API takes seconds on a real project, and the native compiler is
// designed to report the same diagnostics as TypeScript 6.0 (the version check-fixtures.js
// pins), several times faster. With --tsgo compare.js asks tsgo instead of tsc.
//
// How it differs from the tsc side:
//   - tsgo has no JS compiler API in 7.0, so this reads the CLI's text output
//     (`file(line,col): error TS1234: message`, continuation lines indented) instead of
//     ts.Diagnostic objects. The message text is the same flattened text the API gives.
//   - It always checks the directory as ONE project, like compare.js --module: a process per
//     file would pay tsgo's library load (about 0.4 s) for every fixture. --tsgo implies
//     --module, and the target must be a directory.
//   - compare.js runs the tsgo binary itself, through the same measuring wrapper as tsox, so
//     the time and peak memory it reports are tsgo's own and not a Node wrapper's.
//
// The compiler options and the root files are the ones check-fixtures.js uses for --module
// (projectOptions, collectProjectFiles), written to a temporary tsconfig.json, so both
// references see the same project.
//
// Finding the binary, in order: $TSGO_BIN, a `tsgo` on PATH, the copy this tool installed
// under scripts/ts-diag-tool/.tsgo (compare-local.sh --tsgo installs typescript@7 there when
// nothing else is found).

const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const { collectProjectFiles } = require("./check-fixtures");

const INSTALL_DIR = path.join(__dirname, ".tsgo");

// The same settings as check-fixtures.js baseOptions / strictExtras / projectOptions, as the
// strings a tsconfig.json takes.
const BASE_OPTIONS = {
  strict: true,
  noEmit: true,
  skipLibCheck: true,
  target: "es2020",
  module: "preserve",
  moduleResolution: "bundler",
  moduleDetection: "force",
  jsx: "preserve",
};
const STRICT_EXTRAS = {
  allowUnreachableCode: false,
  noUncheckedIndexedAccess: true,
  noImplicitOverride: true,
  exactOptionalPropertyTypes: true,
  noPropertyAccessFromIndexSignature: true,
  noFallthroughCasesInSwitch: true,
  noImplicitReturns: true,
};

function isExecutable(file) {
  try {
    fs.accessSync(file, fs.constants.X_OK);
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

// typescript@7 ships the compiler as an optional platform package,
// node_modules/@typescript/typescript-<platform>-<arch>/lib/tsc.
function findInstalled(root) {
  const scope = path.join(root, "node_modules", "@typescript");
  if (!fs.existsSync(scope)) return null;
  const exe = process.platform === "win32" ? "tsc.exe" : "tsc";
  for (const name of fs.readdirSync(scope)) {
    if (!name.startsWith("typescript-")) continue;
    const candidate = path.join(scope, name, "lib", exe);
    if (isExecutable(candidate)) return candidate;
  }
  return null;
}

function findTsgo() {
  if (process.env.TSGO_BIN) {
    return isExecutable(process.env.TSGO_BIN) ? process.env.TSGO_BIN : null;
  }
  const onPath = spawnSync(
    process.platform === "win32" ? "where" : "which",
    ["tsgo"],
    { encoding: "utf8" },
  );
  if (onPath.status === 0) {
    const first = onPath.stdout.split(/\r?\n/)[0].trim();
    if (first && isExecutable(first)) return first;
  }
  return findInstalled(INSTALL_DIR);
}

// The line `tsgo --version` prints, e.g. "Version 7.0.2".
function tsgoVersion(bin) {
  const res = spawnSync(bin, ["--version"], { encoding: "utf8" });
  return (res.stdout || "").trim() || "unknown";
}

// Writes the temporary tsconfig.json for `target` and returns what compare.js needs to run
// and read tsgo: the roots (relative to cwd, like checkProject), the command arguments, and
// a cleanup function.
function prepareProject(target, { strictOnly }) {
  const files = collectProjectFiles(target);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "tsox-tsgo-"));
  const tsconfig = path.join(dir, "tsconfig.json");
  fs.writeFileSync(
    tsconfig,
    JSON.stringify({
      compilerOptions: strictOnly
        ? BASE_OPTIONS
        : { ...BASE_OPTIONS, ...STRICT_EXTRAS },
      files: files.map((f) => path.resolve(f)),
    }),
  );
  return {
    files: files.map((f) => path.relative(process.cwd(), f)),
    // --extendedDiagnostics adds the Total time line, which is tsgo's own figure for the
    // Program build and check (compare.js shows it as programMs).
    args: ["--pretty", "false", "--extendedDiagnostics", "-p", tsconfig],
    cleanup: () => fs.rmSync(dir, { recursive: true, force: true }),
  };
}

const DIAGNOSTIC_LINE =
  /^(.*?)\((\d+),(\d+)\): (error|warning|suggestion|message) TS(\d+): (.*)$/;
const GLOBAL_LINE = /^(error|warning|suggestion|message) TS\d+: /;
const CATEGORY = {
  error: "Error",
  warning: "Warning",
  suggestion: "Suggestion",
  message: "Message",
};

// Turns tsgo's text output into the shape checkProject returns: every diagnostic that falls
// in one of the roots as { file, line, column, code, category, message }, programMs and
// outsideRoots. A diagnostic whose message has more lines (an elaboration chain) keeps them,
// joined with "\n", as ts.flattenDiagnosticMessageText does. Diagnostics with no file, or in
// a file that is not a root (a package's declaration file), are counted, not reported.
function parseOutput(text, roots) {
  const rootByPath = new Map(
    roots.map((f) => [path.resolve(process.cwd(), f), f]),
  );
  const results = [];
  let outsideRoots = 0;
  let programMs = null;
  let current = null;
  const finish = () => {
    if (current) current.message = current.lines.join("\n");
  };

  for (const line of text.split(/\r?\n/)) {
    const m = DIAGNOSTIC_LINE.exec(line);
    if (m) {
      finish();
      const root = rootByPath.get(path.resolve(process.cwd(), m[1]));
      if (!root) {
        outsideRoots++;
        current = null;
        continue;
      }
      current = {
        file: root,
        line: Number(m[2]),
        column: Number(m[3]),
        code: Number(m[5]),
        category: CATEGORY[m[4]],
        lines: [m[6]],
      };
      results.push(current);
      continue;
    }
    if (GLOBAL_LINE.test(line)) {
      finish();
      outsideRoots++;
      current = null;
      continue;
    }
    // An indented line continues the message of the diagnostic above it.
    if (current && /^\s+\S/.test(line)) {
      current.lines.push(line);
      continue;
    }
    finish();
    current = null;
    const t = /^Total time:\s+([\d.]+)s/.exec(line);
    if (t) programMs = Number(t[1]) * 1000;
  }
  finish();
  for (const d of results) delete d.lines;
  return { results, programMs, outsideRoots };
}

module.exports = { findTsgo, tsgoVersion, prepareProject, parseOutput, INSTALL_DIR };
