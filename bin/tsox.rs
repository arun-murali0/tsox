use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use tsox::{Diagnostic, LineIndex, Severity, TypeChecker};

// mimalloc runs only in this binary, and only in a build with the `mimalloc` feature. It
// is set here and not in the library because a library that chose a global allocator
// would change it for every program that links the crate. WASM has no use for it and
// keeps the platform allocator. Compare timings only between builds that use the same
// allocator.
#[cfg(all(feature = "mimalloc", not(target_arch = "wasm32")))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

// `flat` and `cycles` only change behavior in a build with the `module-resolution`
// feature; a build without it always checks each file on its own.
#[cfg_attr(not(feature = "module-resolution"), allow(dead_code))]
struct Options {
    project: String,
    flat: bool,
    cycles: bool,
    partial: bool,
}

// Set once from `--tsc-codes`: print each diagnostic under the tsc code it maps to.
static TSC_CODES: AtomicBool = AtomicBool::new(false);

// What a run found, kept apart from printing so both ways of checking (file by file,
// or through the module graph) report in the same shape.
#[derive(Default)]
struct Tally {
    files: usize,
    errors: usize,
    warnings: usize,
    fatal: bool,
    note: Option<String>,
}

impl Tally {
    fn print(&mut self, diagnostics: &[Diagnostic], source: &str) {
        let line_index = LineIndex::new(source);
        for diagnostic in diagnostics {
            match diagnostic.severity {
                Severity::Error => self.errors += 1,
                Severity::Warning => self.warnings += 1,
            }
            println!(
                "{}",
                diagnostic.format_with_code(&line_index, source, TSC_CODES.load(Ordering::Relaxed))
            );
        }
    }

    fn fail(&mut self, message: &str) {
        eprintln!("error: {message}");
        self.fatal = true;
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    let Some(options) = parse_options(&args) else {
        eprintln!("error: expected --project <path-to-tsconfig.json>");
        print_help();
        return ExitCode::from(2);
    };

    TSC_CODES.store(
        args.iter().any(|arg| arg == "--tsc-codes"),
        Ordering::Relaxed,
    );

    let tsconfig_path = PathBuf::from(&options.project);
    if !tsconfig_path.is_file() {
        eprintln!("error: no such tsconfig file: {}", tsconfig_path.display());
        return ExitCode::from(2);
    }

    // tsconfig.json's contents are never parsed; the path is only used to locate
    // the project root, and every .ts/.tsx file under that root is then checked
    // directly. There is no include/exclude/paths handling and no support for
    // multiple projects yet. Those files are the entries of the module graph when
    // it is used; files they import are added to the graph from there.
    let Some(project_root) = tsconfig_path.parent() else {
        eprintln!(
            "error: could not determine a project root from {}",
            tsconfig_path.display()
        );
        return ExitCode::from(2);
    };

    let mut source_files = Vec::new();
    collect_ts_files(project_root, &mut source_files);
    source_files.sort();

    if source_files.is_empty() {
        eprintln!(
            "warning: no .ts/.tsx files found under {}",
            project_root.display()
        );
        return ExitCode::SUCCESS;
    }

    let tally = match run(&source_files, &options) {
        Ok(tally) => tally,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    println!();
    println!(
        "Checked {} file(s): {} error(s), {} warning(s).",
        tally.files, tally.errors, tally.warnings
    );
    if let Some(note) = &tally.note {
        println!("{note}");
    }

    if tally.fatal {
        // A file that failed to read or parse at all is distinguished from a file
        // that parsed fine but reported type errors, so a caller (a CI script,
        // for instance) can tell "the checker itself hit a problem" apart from
        // "the checker ran fine and found real issues."
        ExitCode::from(2)
    } else if tally.errors > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(feature = "module-resolution")]
fn run(files: &[PathBuf], options: &Options) -> Result<Tally, String> {
    if options.flat {
        return Ok(tsox::with_deep_stack(|| check_flat(files)));
    }
    check_with_modules(files, options)
}

#[cfg(not(feature = "module-resolution"))]
fn run(files: &[PathBuf], _options: &Options) -> Result<Tally, String> {
    Ok(tsox::with_deep_stack(|| check_flat(files)))
}

// One file at a time with no knowledge of the others, which is what every build did
// before the module graph and is all a build without `module-resolution` can do.
fn check_flat(files: &[PathBuf]) -> Tally {
    let checker = TypeChecker::new();
    let mut tally = Tally {
        files: files.len(),
        ..Tally::default()
    };

    for file_path in files {
        let source = match fs::read_to_string(file_path) {
            Ok(source) => source,
            Err(read_err) => {
                tally.fail(&format!(
                    "could not read {}: {read_err}",
                    file_path.display()
                ));
                continue;
            }
        };

        let file_name = file_path.to_string_lossy().into_owned();
        match checker.check_source(&source, &file_name) {
            Ok(result) => tally.print(&result.diagnostics, &source),
            Err(parse_err) => tally.fail(&format!("{file_name}: {parse_err}")),
        }
    }
    tally
}

// Builds the module graph from the project's files, then checks the graph layer by
// layer with the files of a layer in parallel. Each file is still checked on its own:
// the graph orders the work and finds imports that resolve to nothing, but what a file
// imports is not yet visible to the checker.
#[cfg(feature = "module-resolution")]
fn check_with_modules(files: &[PathBuf], options: &Options) -> Result<Tally, String> {
    use tsox::{FileOutcome, ModuleGraph, ModuleResolver, check_project};

    // The graph wants absolute entry paths so a file reached by an import and the same
    // file named as an entry are one node.
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        let absolute = fs::canonicalize(file)
            .map_err(|error| format!("could not resolve {}: {error}", file.display()))?;
        entries.push(absolute);
    }

    let resolver = ModuleResolver::new();
    let graph = ModuleGraph::build(&entries, &resolver).map_err(|error| error.to_string())?;
    let report = check_project(&graph);

    let mut tally = Tally::default();
    for file_report in &report.files {
        let Some(path) = graph.files().path(file_report.file_id) else {
            continue;
        };
        tally.files += 1;
        match &file_report.outcome {
            FileOutcome::Checked(diagnostics) => match fs::read_to_string(path) {
                Ok(source) => tally.print(diagnostics, &source),
                Err(error) => tally.fail(&format!("could not read {}: {error}", path.display())),
            },
            FileOutcome::ReadFailed(message) => tally.fail(message),
            FileOutcome::CheckFailed(error) => tally.fail(&format!("{}: {error}", path.display())),
        }
    }

    // A warning and not an error: tsconfig's `paths` and `baseUrl` are not read, so an
    // import they would resolve shows up here although tsc would find it.
    let mut unresolved = 0usize;
    for (id, edge) in graph.unresolved() {
        if !graph.is_checkable(id) {
            continue;
        }
        let Some(path) = graph.files().path(id) else {
            continue;
        };
        unresolved += 1;
        tally.warnings += 1;
        println!(
            "{}: warning: cannot find module '{}'",
            path.display(),
            edge.specifier
        );
    }

    // A warning and not an error: the cycle's files were checked, but what they export was
    // still changing when the recheck bound ran out, so an import from them may lack names.
    for id in &report.unsettled {
        let Some(path) = graph.files().path(*id) else {
            continue;
        };
        tally.warnings += 1;
        println!(
            "{}: warning: import cycle did not settle, exports from this file may be incomplete",
            path.display()
        );
    }

    if options.partial {
        for file in &report.files {
            if file.incomplete_reasons.is_empty() {
                continue;
            }
            let Some(path) = graph.files().path(file.file_id) else {
                continue;
            };
            let why: Vec<&str> = file
                .incomplete_reasons
                .iter()
                .map(|reason| reason.describe())
                .collect();
            println!("{}: exports partial: {}", path.display(), why.join("; "));
        }
        for line in tsox::summarize_partial(&graph, &report).lines() {
            println!("{line}");
        }
    }

    let cycles = graph.cycles();
    if options.cycles {
        for cycle in &cycles {
            let members: Vec<String> = cycle
                .iter()
                .filter_map(|id| graph.files().path(*id))
                .map(|path| path.display().to_string())
                .collect();
            println!("cycle: {}", members.join(", "));
        }
    }

    tally.note = Some(format!(
        "Module graph: {} file(s), {} cycle(s), {} unresolved import(s).",
        graph.len(),
        cycles.len(),
        unresolved
    ));
    Ok(tally)
}

fn parse_options(args: &[String]) -> Option<Options> {
    let project = parse_project_arg(args)?;
    Some(Options {
        project,
        flat: args.iter().any(|arg| arg == "--flat"),
        cycles: args.iter().any(|arg| arg == "--cycles"),
        partial: args.iter().any(|arg| arg == "--partial"),
    })
}

fn parse_project_arg(args: &[String]) -> Option<String> {
    for (i, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--project=") {
            return Some(value.to_string());
        }
        if arg == "--project" {
            return args.get(i + 1).cloned();
        }
    }
    None
}

fn collect_ts_files(dir: &Path, out: &mut Vec<PathBuf>) {
    // A deterministic, hardcoded skip list rather than reading tsconfig's own
    // exclude patterns, consistent with this CLI not parsing tsconfig.json at
    // all. These are the directories real-world TypeScript projects almost
    // always want excluded regardless of their own configuration.
    const SKIPPED_DIRS: &[&str] = &["node_modules", "dist", "build", "out", "coverage", ".git"];

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if path.is_dir() {
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name) {
                continue;
            }
            collect_ts_files(&path, out);
            continue;
        }

        let is_declaration_file = name.ends_with(".d.ts");
        let is_ts_source = name.ends_with(".ts") || name.ends_with(".tsx");
        if is_ts_source && !is_declaration_file {
            out.push(path);
        }
    }
}

fn print_help() {
    println!(
        "tsox: a from-scratch TypeScript type checker (experimental CLI)\n\n\
         USAGE:\n    \
         tsox --project <path-to-tsconfig.json> [--flat] [--cycles] [--partial] [--tsc-codes]\n\n\
         OPTIONS:\n    \
         --project <path>    Locate the project root from a tsconfig.json path.\n                         \
         Its contents are not parsed; every .ts/.tsx file under\n                         \
         that directory is checked directly. See bin/tsox.rs\n                         \
         for the exact scoping.\n    \
         --flat              Check each file on its own, without the module graph.\n                         \
         The only mode in a build without the `module-resolution`\n                         \
         feature.\n    \
         --cycles            Print each import cycle (module graph builds only).\n    \
         --partial           Print each file whose exports are partial, and why, then a\n                         \
         summary by reason with the project files each one reaches\n                         \
         (module graph builds only).\n    \
         --tsc-codes         Print each diagnostic under the tsc code it maps to\n                         \
         (TS2322) instead of tsox's own (TSR1003).\n    \
         -h, --help          Show this message.\n\n\
         EXIT CODES:\n    \
         0    no errors (warnings may still have been printed)\n    \
         1    one or more type errors were reported\n    \
         2    a file could not be parsed or read at all"
    );
}
