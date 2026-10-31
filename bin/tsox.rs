use std::env;
use std::fs;
use std::path::PathBuf;
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
    // Check through the query layer (`IncrementalProject`): slower on a first run, much faster
    // on every later one. `--watch` implies it.
    incremental: bool,
    watch: bool,
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

    // The config file's directory is the project root. A bare file name such as
    // `tsconfig.json` has an empty parent, which means the current directory.
    let project_root = tsconfig_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));

    // With the module graph, the config is read: `include`, `exclude` and `files` (after
    // `extends`) select the entries, and `paths` and `baseUrl` resolve imports. A config
    // that cannot be read stops the run with exit code 2, as tsc does: checking every file
    // under the root instead would answer a question nobody asked and let a broken config
    // pass a CI gate. There is no support for multiple projects yet. The entries are the
    // roots of the module graph; files they import are added from there.
    #[cfg(feature = "module-resolution")]
    let config = match load_config(&tsconfig_path) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    #[cfg(feature = "module-resolution")]
    let source_files = config.entry_files();
    #[cfg(not(feature = "module-resolution"))]
    let source_files = files_under(project_root);

    // tsc reports no inputs as an error (TS18003), not as a clean run: a config whose
    // include matches nothing is a mistake, and exiting 0 would hide it.
    if source_files.is_empty() {
        eprintln!(
            "error: no .ts/.tsx files selected under {}",
            project_root.display()
        );
        return ExitCode::from(2);
    }

    #[cfg(feature = "module-resolution")]
    let outcome = run(&source_files, &options, Some(&config));
    #[cfg(not(feature = "module-resolution"))]
    let outcome = run(&source_files, &options);
    let tally = match outcome {
        Ok(tally) => tally,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    print_summary(&tally);

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

fn print_summary(tally: &Tally) {
    println!();
    println!(
        "Checked {} file(s): {} error(s), {} warning(s).",
        tally.files, tally.errors, tally.warnings
    );
    if let Some(note) = &tally.note {
        println!("{note}");
    }
}

// Every .ts/.tsx file under `root`, sorted, skipping the usual build and dependency
// directories. What is checked in a build without the module graph, which has no
// configuration to select files.
#[cfg(not(feature = "module-resolution"))]
fn files_under(root: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_ts_files(root, &mut files);
    files.sort();
    files
}

#[cfg(feature = "module-resolution")]
fn load_config(tsconfig_path: &std::path::Path) -> Result<tsox::ProjectConfig, String> {
    let absolute = fs::canonicalize(tsconfig_path)
        .map_err(|error| format!("could not resolve {}: {error}", tsconfig_path.display()))?;
    tsox::ProjectConfig::load(&absolute).map_err(|error| error.to_string())
}

#[cfg(feature = "module-resolution")]
fn run(
    files: &[PathBuf],
    options: &Options,
    config: Option<&tsox::ProjectConfig>,
) -> Result<Tally, String> {
    if options.flat {
        return Ok(tsox::with_deep_stack(|| check_flat(files)));
    }
    if options.incremental {
        return check_incremental(options, config);
    }
    check_with_modules(files, options, config)
}

#[cfg(all(feature = "module-resolution", not(feature = "incremental")))]
fn check_incremental(_: &Options, _: Option<&tsox::ProjectConfig>) -> Result<Tally, String> {
    Err("this build has no `incremental` feature; rebuild with --features incremental".to_owned())
}

// The query layer: the same checker, with every unit's result kept, so a later check runs only
// what an edit reaches. By itself (`--incremental`) one run is the first run and costs more than
// the default; `--watch` is where it pays, checking again whenever a file changes.
#[cfg(all(feature = "module-resolution", feature = "incremental"))]
fn check_incremental(
    options: &Options,
    config: Option<&tsox::ProjectConfig>,
) -> Result<Tally, String> {
    let config = config.ok_or("--incremental needs a tsconfig.json")?;
    let mut project = tsox::IncrementalProject::open(config.config_file())?;
    let report = project.check()?;
    let bundle = project.graph();
    let tally = report_project(bundle.module_graph(), &report, options);
    if !options.watch {
        return Ok(tally);
    }
    print_summary(&tally);
    eprintln!("watching for changes (Ctrl-C to stop)");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if project.entries_changed() {
            if let Err(message) = project.rebuild() {
                eprintln!("error: {message}");
                continue;
            }
        }
        let report = match project.check() {
            Ok(report) => report,
            Err(message) => {
                eprintln!("error: {message}");
                continue;
            }
        };
        let stats = project.stats();
        // Report when something a reader can see may differ: the graph was rebuilt (a file
        // appeared or went, or an import or the config changed), an input's bytes changed, or
        // the report is not the one printed last. Counting checked units is not that test: a
        // removed file nothing imports, or a move that changes no unit's inputs, checks zero
        // units and still changes what the project reports.
        if !stats.changed {
            continue;
        }
        let bundle = project.graph();
        println!(
            "\n--- change detected: {} of {} unit(s) checked, {} reused{} ---",
            stats.units_checked,
            stats.units_total,
            stats.units_reused,
            if stats.rebuilt { ", graph rebuilt" } else { "" }
        );
        let tally = report_project(bundle.module_graph(), &report, options);
        print_summary(&tally);
    }
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
// layer with the files of a layer in parallel. The graph orders the work and finds
// imports that resolve to nothing; a file sees what it imports through the exports the
// files below it published.
#[cfg(feature = "module-resolution")]
fn check_with_modules(
    files: &[PathBuf],
    options: &Options,
    config: Option<&tsox::ProjectConfig>,
) -> Result<Tally, String> {
    use tsox::{ModuleGraph, ModuleResolver, check_project};

    // The graph wants absolute entry paths so a file reached by an import and the same
    // file named as an entry are one node.
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        let absolute = fs::canonicalize(file)
            .map_err(|error| format!("could not resolve {}: {error}", file.display()))?;
        entries.push(absolute);
    }

    // The project's tsconfig.json decides `paths`, `baseUrl` and `extends` for resolution,
    // and is attached to the graph with the project root, so a missing package is reported
    // when nothing in the configuration could answer it. Without a readable config the
    // resolver reads none and there is no root, so a missing package is never reported.
    let (resolver, root) = match config {
        Some(config) => (
            ModuleResolver::with_tsconfig(config.config_file())
                .with_node_next(config.uses_node_next()),
            Some(config.root().to_path_buf()),
        ),
        None => (ModuleResolver::new(), None),
    };
    // The declaration files `types` and `typeRoots` bring in are entries too: nothing imports
    // them, yet their global declarations and ambient modules are part of the program.
    let mut loaded_type_entries = false;
    if let Some(config) = config {
        for path in config.type_entry_files(&resolver) {
            if let Ok(absolute) = fs::canonicalize(&path) {
                if !entries.contains(&absolute) {
                    entries.push(absolute);
                }
            }
        }
        loaded_type_entries = true;
    }
    let mut graph = ModuleGraph::build(&entries, &resolver).map_err(|error| error.to_string())?;
    if let (Some(config), Some(root)) = (config, root) {
        graph = graph.with_root(root).with_config(config);
        if loaded_type_entries {
            graph = graph.with_type_entries_loaded();
        }
    }
    let report = check_project(&graph);
    Ok(report_project(&graph, &report, options))
}

// Prints what a project check found, in the order both ways of checking share, and counts it.
#[cfg(feature = "module-resolution")]
fn report_project(
    graph: &tsox::ModuleGraph,
    report: &tsox::ProjectReport,
    options: &Options,
) -> Tally {
    use tsox::FileOutcome;

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

    // A warning and not an error: tsconfig's `paths`, `baseUrl`, `extends`, `types` and
    // `typeRoots` are applied, but `rootDirs` is not, so an import it would resolve can still
    // show up here although tsc would find it.
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
        for line in tsox::summarize_partial(graph, report).lines() {
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
    tally
}

fn parse_options(args: &[String]) -> Option<Options> {
    let project = parse_project_arg(args)?;
    Some(Options {
        project,
        flat: args.iter().any(|arg| arg == "--flat"),
        cycles: args.iter().any(|arg| arg == "--cycles"),
        partial: args.iter().any(|arg| arg == "--partial"),
        incremental: args
            .iter()
            .any(|arg| arg == "--incremental" || arg == "--watch"),
        watch: args.iter().any(|arg| arg == "--watch"),
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

#[cfg(not(feature = "module-resolution"))]
fn collect_ts_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
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
         tsox --project <path-to-tsconfig.json> [--flat] [--cycles] [--partial] [--tsc-codes]\n    \
         [--incremental | --watch]\n\n\
         OPTIONS:\n    \
         --project <path>    Read the tsconfig.json at <path> and check the files its\n                         \
         include/exclude/files select, resolving imports with its\n                         \
         paths, baseUrl, extends and references. If the config cannot\n                         \
         be read, warn and check every .ts/.tsx file under its directory.\n    \
         --flat              Check each file on its own, without the module graph.\n                         \
         The only mode in a build without the `module-resolution`\n                         \
         feature.\n    \
         --cycles            Print each import cycle (module graph builds only).\n    \
         --partial           Print each file whose exports are partial, and why, then a\n                         \
         summary by reason with the project files each one reaches\n                         \
         (module graph builds only).\n    \
         --tsc-codes         Print each diagnostic under the tsc code it maps to\n                         \
         (TS2322) instead of tsox's own (TSR1003).\n    \
         --incremental       Check through the query layer. One run costs about twice the\n                         \
         default; it exists for --watch and for tools that keep it.\n    \
         --watch             Check, then check again whenever a file changes, running only\n                         \
         what the change reaches (implies --incremental).\n    \
         -h, --help          Show this message.\n\n\
         EXIT CODES:\n    \
         0    no errors (warnings may still have been printed)\n    \
         1    one or more type errors were reported\n    \
         2    a file could not be parsed or read at all"
    );
}
