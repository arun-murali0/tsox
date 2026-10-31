#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{
    CheckOptions, FileId, FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, UnitContext,
    Worker, check_project,
};

// Problem: the checker had one fixed behavior, so a tsconfig that turned off
// `noUncheckedIndexedAccess` or turned on `allowUnreachableCode` or
// `useUnknownInCatchVariables` changed nothing.
// Picked: `CheckOptions` carries the three keys the checker reads. The default is the old
// fixed behavior, so an empty tsconfig (what the comparison harness writes) is unchanged,
// and only a key the tsconfig sets explicitly overrides it.
// Now: the tests below pin the default, each override, and the path from a tsconfig.

const INDEXED: &str = "const xs: number[] = [1];\nconst n: number = xs[0];\n";
const UNREACHABLE: &str = "function f(): number {\n  return 1;\n  const x = 2;\n}\n";
const CATCH: &str = "try {\n  throw 1;\n} catch (e) {\n  const n: number = e;\n}\n";

fn count(source: &str, options: CheckOptions) -> usize {
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(options);
    worker
        .check_unit(&ctx)
        .expect("the unit checks")
        .diagnostics
        .iter()
        .filter(|d| d.severity == tsox::Severity::Error)
        .count()
}

#[test]
fn the_default_keeps_the_fixed_behavior() {
    let defaults = CheckOptions::default();
    assert_eq!(count(INDEXED, defaults), 1, "xs[0] is number | undefined");
    assert_eq!(count(UNREACHABLE, defaults), 1, "dead code is reported");
    assert_eq!(count(CATCH, defaults), 0, "a catch variable is any");
}

#[test]
fn indexed_access_without_undefined_when_the_option_is_off() {
    let options = CheckOptions {
        no_unchecked_indexed_access: false,
        ..CheckOptions::default()
    };
    assert_eq!(count(INDEXED, options), 0);
}

#[test]
fn unreachable_code_is_allowed_when_the_option_says_so() {
    let options = CheckOptions {
        allow_unreachable_code: true,
        ..CheckOptions::default()
    };
    assert_eq!(count(UNREACHABLE, options), 0);
}

#[test]
fn a_catch_variable_is_unknown_when_the_option_says_so() {
    let options = CheckOptions {
        use_unknown_in_catch_variables: true,
        ..CheckOptions::default()
    };
    assert_eq!(
        count(CATCH, options),
        1,
        "unknown is not assignable to number"
    );
}

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(test: &str, tsconfig: &str, source: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("check-options-{test}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("tsconfig.json"), tsconfig).unwrap();
        fs::write(root.join("main.ts"), source).unwrap();
        Self { root }
    }

    fn errors(&self) -> usize {
        let config = ProjectConfig::load(self.root.join("tsconfig.json")).expect("config loads");
        let entries = config.entry_files();
        let resolver = ModuleResolver::with_tsconfig(config.config_file());
        let graph = ModuleGraph::build(&entries, &resolver)
            .expect("graph builds")
            .with_root(config.root().to_path_buf())
            .with_config(&config);
        let report = check_project(&graph);
        report
            .files
            .iter()
            .map(|file| match &file.outcome {
                FileOutcome::Checked(diagnostics) => diagnostics
                    .iter()
                    .filter(|d| d.severity == tsox::Severity::Error)
                    .count(),
                _ => 0,
            })
            .sum()
    }
}

#[test]
fn an_empty_tsconfig_checks_as_before() {
    assert_eq!(Project::new("empty", "{}", INDEXED).errors(), 1);
    assert_eq!(Project::new("empty-dead", "{}", UNREACHABLE).errors(), 1);
}

#[test]
fn a_tsconfig_key_reaches_the_checker() {
    let off = r#"{"compilerOptions":{"noUncheckedIndexedAccess":false}}"#;
    assert_eq!(Project::new("indexed-off", off, INDEXED).errors(), 0);
    let allow = r#"{"compilerOptions":{"allowUnreachableCode":true}}"#;
    assert_eq!(Project::new("dead-allowed", allow, UNREACHABLE).errors(), 0);
    let unknown = r#"{"compilerOptions":{"useUnknownInCatchVariables":true}}"#;
    assert_eq!(Project::new("catch-unknown", unknown, CATCH).errors(), 1);
}

#[test]
fn an_extended_config_key_applies() {
    let project = Project::new("extends", r#"{"extends":"./base.json"}"#, INDEXED);
    fs::write(
        project.root.join("base.json"),
        r#"{"compilerOptions":{"noUncheckedIndexedAccess":false}}"#,
    )
    .unwrap();
    assert_eq!(project.errors(), 0);
}
