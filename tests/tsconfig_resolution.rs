#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};

use tsox::{ModuleGraph, ModuleResolver};

// Problem: the project driver never read tsconfig.json, so an import through `paths` or
// `baseUrl` could not resolve and showed up as a missing module although tsc finds it.
// Picked: ModuleResolver::with_tsconfig hands the config file to the resolver, which
// applies `paths`, `baseUrl`, `extends` chains and project references.
// Now: each of those resolves through the configuration, the same imports stay unresolved
// without it, and a specifier the configuration does not map is still unresolved.

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(test: &str, files: &[(&str, &str)]) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("tsconfig-{test}"));
        let _ = fs::remove_dir_all(&root);
        for (name, text) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn graph(&self, entry: &str, resolver: &ModuleResolver) -> ModuleGraph {
        ModuleGraph::build(&[self.path(entry)], resolver)
            .expect("the project builds")
            .with_root(&self.root)
    }

    fn with_config(&self, entry: &str) -> ModuleGraph {
        let resolver = ModuleResolver::with_tsconfig(self.path("tsconfig.json"));
        self.graph(entry, &resolver)
    }

    fn without_config(&self, entry: &str) -> ModuleGraph {
        self.graph(entry, &ModuleResolver::new())
    }
}

fn unresolved(graph: &ModuleGraph) -> Vec<String> {
    let mut all: Vec<String> = graph
        .unresolved()
        .map(|(_, edge)| edge.specifier.clone())
        .collect();
    all.sort();
    all
}

fn reaches(graph: &ModuleGraph, project: &Project, entry: &str, target: &str) -> bool {
    let from = graph.files().id(&project.path(entry)).expect("entry");
    let to = graph.files().id(&project.path(target));
    to.is_some_and(|to| graph.dependencies(from).contains(&to))
}

#[test]
fn a_paths_alias_with_a_base_url_resolves_through_the_config() {
    let project = Project::new(
        "paths-base-url",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"] } } }"#,
            ),
            (
                "src/app.ts",
                "import { util } from '@lib/util';\nexport const a = util;\n",
            ),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), Vec::<String>::new());
    assert!(reaches(&graph, &project, "src/app.ts", "src/lib/util.ts"));

    // Without the config the same import is a missing module.
    assert_eq!(
        unresolved(&project.without_config("src/app.ts")),
        vec!["@lib/util".to_owned()]
    );
}

#[test]
fn an_exact_paths_entry_maps_one_specifier_to_one_file() {
    let project = Project::new(
        "paths-exact",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "config": ["src/settings/config.ts"] } } }"#,
            ),
            (
                "src/app.ts",
                "import { c } from 'config';\nexport const a = c;\n",
            ),
            ("src/settings/config.ts", "export const c = 1;\n"),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), Vec::<String>::new());
    assert!(reaches(
        &graph,
        &project,
        "src/app.ts",
        "src/settings/config.ts"
    ));
}

#[test]
fn base_url_alone_makes_a_bare_specifier_relative_to_it() {
    let project = Project::new(
        "base-url-only",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "baseUrl": "src" } }"#,
            ),
            (
                "src/app.ts",
                "import { util } from 'lib/util';\nexport const a = util;\n",
            ),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), Vec::<String>::new());
    assert!(reaches(&graph, &project, "src/app.ts", "src/lib/util.ts"));
}

#[test]
fn paths_without_a_base_url_are_relative_to_the_config_file() {
    let project = Project::new(
        "paths-no-base-url",
        &[
            (
                "tsconfig.json",
                r##"{ "compilerOptions": { "paths": { "#shared/*": ["./shared/*"] } } }"##,
            ),
            (
                "src/app.ts",
                "import { s } from '#shared/s';\nexport const a = s;\n",
            ),
            ("shared/s.ts", "export const s = 1;\n"),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), Vec::<String>::new());
    assert!(reaches(&graph, &project, "src/app.ts", "shared/s.ts"));
}

#[test]
fn paths_inherited_through_extends_resolve() {
    let project = Project::new(
        "extends",
        &[
            (
                "tsconfig.base.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"] } } }"#,
            ),
            ("tsconfig.json", r#"{ "extends": "./tsconfig.base.json" }"#),
            (
                "src/app.ts",
                "import { util } from '@lib/util';\nexport const a = util;\n",
            ),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), Vec::<String>::new());
    assert!(reaches(&graph, &project, "src/app.ts", "src/lib/util.ts"));
}

#[test]
fn a_config_with_comments_and_trailing_commas_is_read() {
    let project = Project::new(
        "jsonc",
        &[
            (
                "tsconfig.json",
                "{\n  // aliases\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    /* one alias */\n    \"paths\": { \"@lib/*\": [\"src/lib/*\",], },\n  },\n}\n",
            ),
            (
                "src/app.ts",
                "import { util } from '@lib/util';\nexport const a = util;\n",
            ),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), Vec::<String>::new());
}

#[test]
fn a_specifier_the_config_does_not_map_is_still_unresolved() {
    let project = Project::new(
        "unmapped",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"] } } }"#,
            ),
            (
                "src/app.ts",
                "import { util } from '@lib/util';\nimport { x } from '@other/x';\nimport { y } from './nope';\nexport const a = [util, x, y];\n",
            ),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(
        unresolved(&graph),
        vec!["./nope".to_owned(), "@other/x".to_owned()]
    );
}

#[test]
fn a_paths_alias_that_points_nowhere_is_still_unresolved() {
    let project = Project::new(
        "alias-to-nothing",
        &[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"] } } }"#,
            ),
            (
                "src/app.ts",
                "import { util } from '@lib/missing';\nexport const a = util;\n",
            ),
        ],
    );
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), vec!["@lib/missing".to_owned()]);
}

#[test]
fn a_missing_config_file_does_not_stop_the_graph_from_building() {
    let project = Project::new(
        "no-config",
        &[
            (
                "src/app.ts",
                "import { util } from './lib/util';\nexport const a = util;\n",
            ),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    );
    // No tsconfig.json was written. Relative imports resolve as they always did.
    let graph = project.with_config("src/app.ts");
    assert!(reaches(&graph, &project, "src/app.ts", "src/lib/util.ts"));
}

// ---- What the checker does with a missing package once the config is applied.

fn checked_codes(graph: &ModuleGraph, file: &str) -> Vec<u32> {
    let report = tsox::check_project(graph);
    let mut codes = Vec::new();
    for entry in &report.files {
        let path = graph.files().path(entry.file_id).unwrap();
        if path.file_name().unwrap().to_string_lossy() == file {
            match &entry.outcome {
                tsox::FileOutcome::Checked(diagnostics) => {
                    codes.extend(diagnostics.iter().filter_map(|d| d.code.tsc_code()));
                }
                other => panic!("{file} was not checked: {other:?}"),
            }
        }
    }
    codes
}

// The text gate also looks above the project, and a `node_modules/@types` or a config
// with those keys up there would hide the report for a reason this test is not about.
fn something_above_the_project_may_answer(root: &Path) -> bool {
    root.ancestors().skip(1).any(|dir| {
        dir.join("node_modules").join("@types").is_dir()
            || ["tsconfig.json", "jsconfig.json"].iter().any(|name| {
                fs::read_to_string(dir.join(name)).is_ok_and(|text| {
                    [
                        "\"paths\"",
                        "\"baseUrl\"",
                        "\"rootDirs\"",
                        "\"extends\"",
                        "\"typeRoots\"",
                    ]
                    .iter()
                    .any(|key| text.contains(key))
                })
            })
    })
}

const APP: &str = "import { util } from '@lib/util';\nimport { pad } from 'left-pad';\nexport const a = [util, pad];\n";
const PATHS: &str = r#""baseUrl": ".", "paths": { "@lib/*": ["src/lib/*"] }"#;

fn alias_project(test: &str, config: &str) -> Project {
    Project::new(
        test,
        &[
            ("tsconfig.json", config),
            ("src/app.ts", APP),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    )
}

#[test]
fn with_the_config_applied_a_missing_package_is_ts2307_although_the_config_has_paths() {
    let project = alias_project(
        "gate-open",
        &format!(r#"{{ "compilerOptions": {{ {PATHS} }} }}"#),
    );
    if something_above_the_project_may_answer(&project.root) {
        return;
    }
    let graph = project.with_config("src/app.ts");
    assert!(graph.tsconfig_applied());
    // `@lib/util` resolved through the config, so the one missing module is `left-pad`.
    assert_eq!(unresolved(&graph), vec!["left-pad".to_owned()]);
    assert_eq!(checked_codes(&graph, "app.ts"), vec![2307]);
}

#[test]
fn without_the_config_applied_the_text_gate_still_hides_a_package() {
    let project = alias_project(
        "gate-closed-no-config",
        &format!(r#"{{ "compilerOptions": {{ {PATHS} }} }}"#),
    );
    if something_above_the_project_may_answer(&project.root) {
        return;
    }
    let graph = project.without_config("src/app.ts");
    assert!(!graph.tsconfig_applied());
    // Both bare specifiers are unresolved, and neither is reported: `paths` might map them.
    assert_eq!(checked_codes(&graph, "app.ts"), Vec::<u32>::new());
}

#[test]
fn a_config_with_root_dirs_keeps_the_gate_closed_even_when_applied() {
    let project = alias_project(
        "gate-root-dirs",
        &format!(r#"{{ "compilerOptions": {{ {PATHS}, "rootDirs": ["src", "gen"] }} }}"#),
    );
    if something_above_the_project_may_answer(&project.root) {
        return;
    }
    let graph = project.with_config("src/app.ts");
    assert_eq!(checked_codes(&graph, "app.ts"), Vec::<u32>::new());
}

#[test]
fn a_config_that_extends_another_keeps_the_gate_closed_even_when_applied() {
    let project = Project::new(
        "gate-extends",
        &[
            (
                "tsconfig.base.json",
                &format!(r#"{{ "compilerOptions": {{ {PATHS} }} }}"#),
            ),
            ("tsconfig.json", r#"{ "extends": "./tsconfig.base.json" }"#),
            ("src/app.ts", APP),
            ("src/lib/util.ts", "export const util = 1;\n"),
        ],
    );
    if something_above_the_project_may_answer(&project.root) {
        return;
    }
    let graph = project.with_config("src/app.ts");
    assert_eq!(unresolved(&graph), vec!["left-pad".to_owned()]);
    assert_eq!(checked_codes(&graph, "app.ts"), Vec::<u32>::new());
}
