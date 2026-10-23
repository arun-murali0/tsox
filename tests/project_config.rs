#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{
    ConfigError, FileOutcome, Json, ModuleGraph, ModuleResolver, ProjectConfig, Severity,
    check_project, parse_jsonc,
};

// Problem: the engine took every `.ts` file under a directory and read the config only as
// text, so `include`, `exclude` and `files` selected nothing, an `extends` chain was
// invisible, a change to the config changed nothing the graph could see, and `exports`
// conditions ignored whether an import was a `require`.
// Picked: `ProjectConfig` reads the config and what it extends and selects the files the
// way TypeScript does. `ModuleGraph::with_config` records the config files as an input and
// lets the effective options decide the missing-package gate. `ResolveMode` reads a
// package's `exports` under `require` for `import x = require()` and for `.cts` importers.
// Now: each of those behaves as the tests below say.

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(test: &str, files: &[(&str, &str)]) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("project-config-{test}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let fixture = Self { root };
        for (name, text) in files {
            fixture.write(name, text);
        }
        fixture
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn config(&self) -> ProjectConfig {
        ProjectConfig::load(self.path("tsconfig.json")).expect("the config loads")
    }

    // The selected files, relative to the root, with `/` separators.
    fn selected(&self) -> Vec<String> {
        self.config()
            .entry_files()
            .iter()
            .map(|path| {
                path.strip_prefix(&self.root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    // The way the command line builds a graph: entries from the config, the config's
    // resolver, the root, and the config attached.
    fn graph(&self, entries: &[&str]) -> ModuleGraph {
        let config = self.config();
        let paths: Vec<PathBuf> = entries.iter().map(|e| self.path(e)).collect();
        ModuleGraph::build(&paths, &ModuleResolver::with_tsconfig(config.config_file()))
            .expect("the project builds")
            .with_root(&self.root)
            .with_config(&config)
    }
}

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn error_codes(graph: &ModuleGraph, file: &str) -> Vec<Option<u32>> {
    let report = check_project(graph);
    for entry in &report.files {
        let path = graph.files().path(entry.file_id).unwrap();
        if path.file_name().unwrap().to_string_lossy() == file {
            return match &entry.outcome {
                FileOutcome::Checked(diagnostics) => diagnostics
                    .iter()
                    .filter(|d| d.severity == Severity::Error)
                    .map(|d| d.code.tsc_code())
                    .collect(),
                other => panic!("{file} was not checked: {other:?}"),
            };
        }
    }
    panic!("{file} is not in the report");
}

// ---- reading the config

#[test]
fn a_config_is_json_with_comments_and_trailing_commas() {
    let value = parse_jsonc("{ // c\n \"a\": [1,], }").unwrap();
    assert_eq!(
        value,
        Json::Object(vec![("a".to_owned(), Json::Array(vec![Json::Number(1.0)]))])
    );
}

#[test]
fn a_config_that_cannot_be_read_is_an_error_not_an_empty_project() {
    let missing = Fixture::new("cfg-missing", &[]);
    assert!(matches!(
        ProjectConfig::load(missing.path("tsconfig.json")),
        Err(ConfigError::Read { .. })
    ));

    let bad = Fixture::new("cfg-bad", &[("tsconfig.json", "{ \"include\": [ }")]);
    assert!(matches!(
        ProjectConfig::load(bad.path("tsconfig.json")),
        Err(ConfigError::Parse { .. })
    ));

    let not_object = Fixture::new("cfg-array", &[("tsconfig.json", "[]")]);
    assert!(matches!(
        ProjectConfig::load(not_object.path("tsconfig.json")),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn extends_that_names_nothing_or_loops_is_an_error() {
    let gone = Fixture::new(
        "extends-gone",
        &[("tsconfig.json", r#"{ "extends": "./nope.json" }"#)],
    );
    assert!(matches!(
        ProjectConfig::load(gone.path("tsconfig.json")),
        Err(ConfigError::ExtendsNotFound { .. })
    ));

    let looped = Fixture::new(
        "extends-loop",
        &[
            ("tsconfig.json", r#"{ "extends": "./a.json" }"#),
            ("a.json", r#"{ "extends": "./tsconfig.json" }"#),
        ],
    );
    assert!(matches!(
        ProjectConfig::load(looped.path("tsconfig.json")),
        Err(ConfigError::ExtendsCycle { .. })
    ));
}

#[test]
fn compiler_options_merge_key_by_key_and_the_extending_file_wins() {
    let fixture = Fixture::new(
        "options-merge",
        &[
            (
                "base.json",
                r#"{ "compilerOptions": { "strict": true, "target": "es5" } }"#,
            ),
            (
                "tsconfig.json",
                r#"{ "extends": "./base", "compilerOptions": { "target": "es2022" } }"#,
            ),
        ],
    );
    let config = fixture.config();
    assert_eq!(config.compiler_option("strict"), Some(&Json::Bool(true)));
    assert_eq!(
        config.compiler_option("target"),
        Some(&Json::String("es2022".to_owned()))
    );
    assert_eq!(config.compiler_option("absent"), None);
    // The file itself first, then what it extends.
    assert_eq!(config.sources().len(), 2);
    assert!(config.sources()[0].ends_with("tsconfig.json"));
    assert!(config.sources()[1].ends_with("base.json"));
}

#[test]
fn extends_can_name_a_package_config_and_a_list() {
    let fixture = Fixture::new(
        "extends-package",
        &[
            (
                "node_modules/@acme/config/tsconfig.json",
                r#"{ "compilerOptions": { "strict": true } }"#,
            ),
            ("one.json", r#"{ "compilerOptions": { "target": "es5" } }"#),
            (
                "tsconfig.json",
                r#"{ "extends": ["@acme/config/tsconfig.json", "./one.json"] }"#,
            ),
        ],
    );
    let config = fixture.config();
    assert_eq!(config.compiler_option("strict"), Some(&Json::Bool(true)));
    assert_eq!(
        config.compiler_option("target"),
        Some(&Json::String("es5".to_owned()))
    );
}

// ---- selecting files

#[test]
fn with_no_include_or_files_everything_under_the_config_is_selected() {
    let fixture = Fixture::new(
        "select-all",
        &[
            ("tsconfig.json", "{}"),
            ("a.ts", ""),
            ("src/b.tsx", ""),
            ("src/deep/c.mts", ""),
            ("src/notes.md", ""),
            ("src/script.js", ""),
        ],
    );
    assert_eq!(
        fixture.selected(),
        names(&["a.ts", "src/b.tsx", "src/deep/c.mts"])
    );
}

#[test]
fn node_modules_and_dot_directories_are_not_entered_by_a_wildcard() {
    let fixture = Fixture::new(
        "select-skips",
        &[
            ("tsconfig.json", "{}"),
            ("a.ts", ""),
            ("node_modules/pkg/index.d.ts", ""),
            ("bower_components/x/x.ts", ""),
            (".hidden/h.ts", ""),
            ("src/.cache/c.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["a.ts"]));
}

#[test]
fn the_default_exclude_also_leaves_out_the_out_dir() {
    let fixture = Fixture::new(
        "select-outdir",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "outDir": "build" } }"#,
            ),
            ("a.ts", ""),
            ("build/a.d.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["a.ts"]));
}

#[test]
fn include_selects_by_pattern_and_exclude_removes() {
    let fixture = Fixture::new(
        "select-include",
        &[
            (
                "tsconfig.json",
                r#"{ "include": ["src/**/*", "lib/*.ts"], "exclude": ["src/legacy", "**/*.test.ts"] }"#,
            ),
            ("src/a.ts", ""),
            ("src/a.test.ts", ""),
            ("src/legacy/old.ts", ""),
            ("src/nested/b.ts", ""),
            ("lib/c.ts", ""),
            ("lib/sub/d.ts", ""),
            ("other/e.ts", ""),
        ],
    );
    assert_eq!(
        fixture.selected(),
        names(&["lib/c.ts", "src/a.ts", "src/nested/b.ts"])
    );
}

#[test]
fn an_include_naming_a_directory_selects_what_is_under_it() {
    let fixture = Fixture::new(
        "select-directory",
        &[
            ("tsconfig.json", r#"{ "include": ["src"] }"#),
            ("src/a.ts", ""),
            ("src/deep/b.ts", ""),
            ("other/c.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["src/a.ts", "src/deep/b.ts"]));
}

#[test]
fn a_question_mark_matches_one_character() {
    let fixture = Fixture::new(
        "select-question",
        &[
            ("tsconfig.json", r#"{ "include": ["src/f?.ts"] }"#),
            ("src/f1.ts", ""),
            ("src/f22.ts", ""),
            ("src/f.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["src/f1.ts"]));
}

#[test]
fn files_lists_exact_files_and_with_no_include_selects_only_them() {
    let fixture = Fixture::new(
        "select-files",
        &[
            ("tsconfig.json", r#"{ "files": ["main.ts", "gone.ts"] }"#),
            ("main.ts", ""),
            ("other.ts", ""),
        ],
    );
    // A listed file that does not exist is not selected; one that exists is.
    assert_eq!(fixture.selected(), names(&["main.ts"]));
}

#[test]
fn files_and_include_are_both_selected() {
    let fixture = Fixture::new(
        "select-both",
        &[
            (
                "tsconfig.json",
                r#"{ "files": ["main.ts"], "include": ["src"] }"#,
            ),
            ("main.ts", ""),
            ("src/a.ts", ""),
            ("other.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["main.ts", "src/a.ts"]));
}

#[test]
fn exclude_set_by_the_config_replaces_the_default() {
    // Setting `exclude` drops the default list, but a wildcard still never enters
    // `node_modules`, so only an explicit pattern could.
    let fixture = Fixture::new(
        "select-exclude-replaces",
        &[
            ("tsconfig.json", r#"{ "exclude": ["skip"] }"#),
            ("a.ts", ""),
            ("skip/s.ts", ""),
            ("node_modules/p/p.d.ts", ""),
            ("build/b.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["a.ts", "build/b.ts"]));
}

#[test]
fn a_ts_file_hides_the_tsx_and_declaration_file_of_the_same_name() {
    let fixture = Fixture::new(
        "select-priority",
        &[
            ("tsconfig.json", "{}"),
            ("a.ts", ""),
            ("a.d.ts", ""),
            ("b.tsx", ""),
            ("b.d.ts", ""),
            ("c.d.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["a.ts", "b.tsx", "c.d.ts"]));
}

#[test]
fn include_inherited_through_extends_is_relative_to_the_file_that_wrote_it() {
    let fixture = Fixture::new(
        "select-extends-base",
        &[
            ("shared/base.json", r#"{ "include": ["../src"] }"#),
            ("tsconfig.json", r#"{ "extends": "./shared/base.json" }"#),
            ("src/a.ts", ""),
            ("shared/s.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["src/a.ts"]));
}

#[test]
fn an_extending_file_replaces_include_whole() {
    let fixture = Fixture::new(
        "select-extends-override",
        &[
            ("base.json", r#"{ "include": ["one"] }"#),
            (
                "tsconfig.json",
                r#"{ "extends": "./base.json", "include": ["two"] }"#,
            ),
            ("one/a.ts", ""),
            ("two/b.ts", ""),
        ],
    );
    assert_eq!(fixture.selected(), names(&["two/b.ts"]));
}

// ---- the config as an input of the graph

#[test]
fn a_changed_config_file_is_seen_and_an_unchanged_one_is_not() {
    let fixture = Fixture::new(
        "input-change",
        &[
            ("tsconfig.json", r#"{ "extends": "./base.json" }"#),
            ("base.json", r#"{ "compilerOptions": {} }"#),
            ("a.ts", "export const a = 1;\n"),
        ],
    );
    let graph = fixture.graph(&["a.ts"]);
    assert!(!graph.config_changed());
    assert_eq!(graph.config_files().len(), 2);

    // Touching the file without changing its bytes is not a change.
    fixture.write("base.json", r#"{ "compilerOptions": {} }"#);
    assert!(!graph.config_changed());

    // Changing the config, or the file it extends, is.
    fixture.write("base.json", r#"{ "compilerOptions": { "strict": true } }"#);
    assert!(graph.config_changed());

    let again = fixture.graph(&["a.ts"]);
    assert!(!again.config_changed());
    fixture.write("tsconfig.json", r#"{ "compilerOptions": {} }"#);
    assert!(again.config_changed());
}

#[test]
fn a_graph_with_no_config_attached_never_reports_a_config_change() {
    let fixture = Fixture::new(
        "input-none",
        &[("tsconfig.json", "{}"), ("a.ts", "export const a = 1;\n")],
    );
    let graph = ModuleGraph::build(&[fixture.path("a.ts")], &ModuleResolver::new()).unwrap();
    assert!(!graph.config_changed());
    assert!(graph.config_files().is_empty());
    assert_eq!(graph.config_hides_bare_specifiers(), None);
}

// ---- the missing-package gate reads the effective config

const MISSING: &str = "import { X } from 'left-pad';\nexport const y = 1;\n";

fn nothing_above_answers_bare_specifiers(root: &Path) -> bool {
    !root
        .ancestors()
        .any(|dir| dir.join("node_modules").join("@types").is_dir())
}

#[test]
fn a_missing_package_is_reported_in_a_project_with_path_aliases() {
    let fixture = Fixture::new(
        "gate-paths",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"] } } }"#,
            ),
            (
                "src/app.ts",
                "import { u } from '@lib/util';\nimport { X } from 'left-pad';\nexport const a = [u, X];\n",
            ),
            ("src/lib/util.ts", "export const u = 1;\n"),
        ],
    );
    if !nothing_above_answers_bare_specifiers(&fixture.root) {
        return;
    }
    let graph = fixture.graph(&["src/app.ts"]);
    // The alias resolved; the package nothing answers is the only error.
    assert_eq!(error_codes(&graph, "app.ts"), vec![Some(2307)]);
}

#[test]
fn a_config_that_only_extends_does_not_hide_a_missing_package() {
    // Before the effective config was read, the text `"extends"` alone kept the gate shut.
    let fixture = Fixture::new(
        "gate-extends",
        &[
            ("base.json", r#"{ "compilerOptions": { "strict": true } }"#),
            ("tsconfig.json", r#"{ "extends": "./base.json" }"#),
            ("a.ts", MISSING),
        ],
    );
    if !nothing_above_answers_bare_specifiers(&fixture.root) {
        return;
    }
    let graph = fixture.graph(&["a.ts"]);
    assert_eq!(graph.config_hides_bare_specifiers(), Some(false));
    assert_eq!(error_codes(&graph, "a.ts"), vec![Some(2307)]);
}

#[test]
fn type_roots_custom_conditions_and_module_suffixes_hide_a_missing_package_but_root_dirs_do_not() {
    // `typeRoots` can bring in `declare module` declarations the checker does not read, so
    // it keeps the gate shut. `rootDirs` only merges directories for relative imports; it
    // cannot answer a package name, so a package nothing answers is reported.
    for (test, option, hides) in [
        ("gate-root-dirs", r#""rootDirs": ["src", "gen"]"#, false),
        ("gate-type-roots", r#""typeRoots": ["./types"]"#, true),
        // The resolver is never given these two, so either could be what finds a package.
        (
            "gate-custom-conditions",
            r#""customConditions": ["source"]"#,
            true,
        ),
        (
            "gate-module-suffixes",
            r#""moduleSuffixes": [".ios", ""]"#,
            true,
        ),
    ] {
        let fixture = Fixture::new(
            test,
            &[
                (
                    "base.json",
                    &format!(r#"{{ "compilerOptions": {{ {option} }} }}"#),
                ),
                ("tsconfig.json", r#"{ "extends": "./base.json" }"#),
                ("a.ts", MISSING),
            ],
        );
        if !hides && !nothing_above_answers_bare_specifiers(&fixture.root) {
            continue;
        }
        let graph = fixture.graph(&["a.ts"]);
        assert_eq!(graph.config_hides_bare_specifiers(), Some(hides), "{test}");
        let expected = if hides {
            Vec::<Option<u32>>::new()
        } else {
            vec![Some(2307)]
        };
        assert_eq!(error_codes(&graph, "a.ts"), expected, "{test}");
    }
}

#[test]
fn paths_the_resolver_was_not_given_still_hide_a_missing_package() {
    // The config is attached, but the resolver was built without it, so `paths` has not
    // decided anything and could still point at what the checker does not read.
    let fixture = Fixture::new(
        "gate-paths-not-applied",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "left-pad": ["./x"] } } }"#,
            ),
            ("a.ts", MISSING),
        ],
    );
    let config = fixture.config();
    let graph = ModuleGraph::build(&[fixture.path("a.ts")], &ModuleResolver::new())
        .unwrap()
        .with_root(&fixture.root)
        .with_config(&config);
    assert_eq!(graph.config_hides_bare_specifiers(), Some(true));
    assert_eq!(error_codes(&graph, "a.ts"), Vec::<Option<u32>>::new());
}

// ---- `exports` conditions follow the kind of import

const PACKAGE: &[(&str, &str)] = &[
    (
        "node_modules/pkg/package.json",
        r#"{ "name": "pkg", "version": "1.0.0", "exports": { ".": { "import": "./esm.d.ts", "require": "./cjs.d.ts" } } }"#,
    ),
    (
        "node_modules/pkg/esm.d.ts",
        "export declare const kind: 'esm';\n",
    ),
    (
        "node_modules/pkg/cjs.d.ts",
        "export declare const kind: 'cjs';\n",
    ),
];

fn package_project(test: &str, extra: &[(&str, &str)]) -> Fixture {
    let mut files: Vec<(&str, &str)> = PACKAGE.to_vec();
    files.push(("tsconfig.json", "{}"));
    files.extend_from_slice(extra);
    Fixture::new(test, &files)
}

fn target_of(graph: &ModuleGraph, fixture: &Fixture, from: &str) -> String {
    let id = graph.files().id(&fixture.path(from)).unwrap();
    let edge = graph.edges(id).first().expect("one edge");
    let target = edge.target.expect("it resolved");
    graph
        .files()
        .path(target)
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn an_import_statement_reads_the_import_condition() {
    let fixture = package_project(
        "exports-import",
        &[("a.ts", "import { kind } from 'pkg';\n")],
    );
    let graph = fixture.graph(&["a.ts"]);
    assert_eq!(target_of(&graph, &fixture, "a.ts"), "esm.d.ts");
}

#[test]
fn import_equals_require_reads_the_require_condition() {
    let fixture = package_project(
        "exports-require",
        &[("a.ts", "import kind = require('pkg');\n")],
    );
    let graph = fixture.graph(&["a.ts"]);
    assert_eq!(target_of(&graph, &fixture, "a.ts"), "cjs.d.ts");
}

#[test]
fn an_import_in_a_cts_file_reads_the_require_condition() {
    let fixture = package_project("exports-cts", &[("a.cts", "import { kind } from 'pkg';\n")]);
    let graph = fixture.graph(&["a.cts"]);
    assert_eq!(target_of(&graph, &fixture, "a.cts"), "cjs.d.ts");
}

#[test]
fn an_import_in_an_mts_file_reads_the_import_condition() {
    let fixture = package_project("exports-mts", &[("a.mts", "import { kind } from 'pkg';\n")]);
    let graph = fixture.graph(&["a.mts"]);
    assert_eq!(target_of(&graph, &fixture, "a.mts"), "esm.d.ts");
}

// ---- `rootDirs`

#[test]
fn root_dirs_merge_directories_for_relative_imports() {
    let fixture = Fixture::new(
        "root-dirs",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "rootDirs": ["src", "generated"] } }"#,
            ),
            (
                "src/view.ts",
                "import { t } from './template';\nexport const v = t;\n",
            ),
            ("generated/template.ts", "export const t = 1;\n"),
        ],
    );
    let graph = fixture.graph(&["src/view.ts"]);
    let unresolved: Vec<String> = graph
        .unresolved()
        .map(|(_, edge)| edge.specifier.clone())
        .collect();
    assert_eq!(unresolved, Vec::<String>::new(), "rootDirs did not resolve");
}
