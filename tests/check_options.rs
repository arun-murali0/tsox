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
// Picked: direct/single-file checks retain the comparison-harness profile, while an actual
// tsconfig starts from TypeScript defaults and derives strict sub-options from `strict`.
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
fn the_direct_check_default_keeps_the_comparison_harness_profile() {
    let defaults = CheckOptions::default();
    assert_eq!(count(INDEXED, defaults), 1, "xs[0] is number | undefined");
    assert_eq!(count(UNREACHABLE, defaults), 1, "dead code is reported");
    assert_eq!(
        count(CATCH, defaults),
        0,
        "the compatibility profile keeps catch as any"
    );
}

#[test]
fn explicit_typescript_defaults_do_not_enable_harness_extras() {
    let defaults = CheckOptions::typescript_defaults();
    assert_eq!(
        count(INDEXED, defaults),
        0,
        "noUncheckedIndexedAccess defaults to false"
    );
    assert_eq!(
        count(UNREACHABLE, defaults),
        0,
        "omitted allowUnreachableCode suppresses diagnostics"
    );
    assert_eq!(count(CATCH, defaults), 0, "strict defaults to false");
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
fn an_empty_tsconfig_uses_typescript_compiler_defaults() {
    assert_eq!(Project::new("empty", "{}", INDEXED).errors(), 0);
    assert_eq!(Project::new("empty-dead", "{}", UNREACHABLE).errors(), 0);
    assert_eq!(Project::new("empty-catch", "{}", CATCH).errors(), 0);
}

#[test]
fn strict_derives_its_sub_options_and_explicit_values_win() {
    let strict = r#"{"compilerOptions":{"strict":true}}"#;
    assert_eq!(Project::new("strict-catch", strict, CATCH).errors(), 1);

    let overridden = r#"{"compilerOptions":{"strict":true,"useUnknownInCatchVariables":false}}"#;
    assert_eq!(
        Project::new("strict-catch-override", overridden, CATCH).errors(),
        0
    );
}

#[test]
fn strict_derives_the_individual_strict_flags_but_explicit_values_win() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("effective-strict-options");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config_path = root.join("tsconfig.json");
    fs::write(
        &config_path,
        r#"{"compilerOptions":{"strict":true,"strictNullChecks":false,"noImplicitAny":false,"exactOptionalPropertyTypes":true}}"#,
    )
    .unwrap();

    let config = ProjectConfig::load(config_path).unwrap();
    let options = config.effective_compiler_options().check;
    assert!(options.strict);
    assert!(
        !options.strict_null_checks,
        "explicit false overrides strict"
    );
    assert!(!options.no_implicit_any, "explicit false overrides strict");
    assert!(options.no_implicit_this);
    assert!(options.strict_function_types);
    assert!(options.strict_bind_call_apply);
    assert!(options.strict_property_initialization);
    assert!(options.always_strict);
    assert!(options.strict_builtin_iterator_return);
    assert!(options.use_unknown_in_catch_variables);
    assert!(options.exact_optional_property_types);
    assert!(!options.no_unchecked_indexed_access);
    assert!(options.allow_unreachable_code);
}

#[test]
fn target_libraries_and_library_check_flags_are_preserved_in_effective_options() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("effective-options-shape");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let config_path = root.join("tsconfig.json");
    fs::write(
        &config_path,
        r#"{"compilerOptions":{"target":"ES2022","lib":["ES2022","DOM"],"noLib":true,"skipLibCheck":true}}"#,
    )
    .unwrap();

    let config = ProjectConfig::load(config_path).unwrap();
    let options = config.effective_compiler_options();
    assert_eq!(options.target, tsox::ScriptTarget::Es2022);
    assert_eq!(
        options.lib,
        Some(vec!["es2022".to_owned(), "dom".to_owned()])
    );
    assert!(options.no_lib);
    assert!(options.skip_lib_check);
}

#[test]
fn a_tsconfig_key_reaches_the_checker() {
    let off = r#"{"compilerOptions":{"noUncheckedIndexedAccess":false}}"#;
    assert_eq!(Project::new("indexed-off", off, INDEXED).errors(), 0);
    let on = r#"{"compilerOptions":{"noUncheckedIndexedAccess":true}}"#;
    assert_eq!(Project::new("indexed-on", on, INDEXED).errors(), 1);
    let allow = r#"{"compilerOptions":{"allowUnreachableCode":true}}"#;
    assert_eq!(Project::new("dead-allowed", allow, UNREACHABLE).errors(), 0);
    let disallow = r#"{"compilerOptions":{"allowUnreachableCode":false}}"#;
    assert_eq!(
        Project::new("dead-reported", disallow, UNREACHABLE).errors(),
        1
    );
    let unknown = r#"{"compilerOptions":{"useUnknownInCatchVariables":true}}"#;
    assert_eq!(Project::new("catch-unknown", unknown, CATCH).errors(), 1);
}

#[test]
fn an_extended_config_key_applies() {
    let project = Project::new("extends", r#"{"extends":"./base.json"}"#, INDEXED);
    fs::write(
        project.root.join("base.json"),
        r#"{"compilerOptions":{"noUncheckedIndexedAccess":true}}"#,
    )
    .unwrap();
    assert_eq!(project.errors(), 1);
}
