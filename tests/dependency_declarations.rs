//! A dependency's declaration files are checked without the pass that finds their own errors,
//! but only when nothing in the file can need it. The types a project reads from a dependency
//! must be the same either way, so this checks the one case that did differ while the option
//! was built: `declare const answer = 42`, whose type comes from its initializer.
#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity, check_project};

fn errors(root: &Path) -> Vec<(String, Option<u32>)> {
    let config = ProjectConfig::load(root.join("tsconfig.json")).unwrap();
    let resolver = ModuleResolver::with_tsconfig(config.config_file());
    let entries: Vec<_> = config
        .project_entries_with_standard_libraries(&resolver)
        .unwrap()
        .into_iter()
        .map(|path| fs::canonicalize(path).unwrap())
        .collect();
    let graph = ModuleGraph::build(&entries, &resolver)
        .unwrap()
        .with_root(root)
        .with_config(&config);
    let report = check_project(&graph);
    let mut out = Vec::new();
    for file in &report.files {
        let name = graph
            .files()
            .path(file.file_id)
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if let FileOutcome::Checked(diagnostics) = &file.outcome {
            for d in diagnostics {
                if d.severity == Severity::Error {
                    out.push((name.clone(), d.code.tsc_code()));
                }
            }
        }
    }
    out.sort();
    out
}

fn project(name: &str, dependency: &str, main: &str) -> std::path::PathBuf {
    let root =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("dependency-declarations-{name}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("node_modules/dep")).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"target":"es2020","lib":["es2020"],"moduleResolution":"bundler","module":"esnext"},"files":["main.ts"]}"#,
    )
    .unwrap();
    fs::write(
        root.join("node_modules/dep/package.json"),
        r#"{"name":"dep","version":"1.0.0","types":"index.d.ts"}"#,
    )
    .unwrap();
    fs::write(root.join("node_modules/dep/index.d.ts"), dependency).unwrap();
    fs::write(root.join("main.ts"), main).unwrap();
    root
}

#[test]
fn an_ambient_initializer_in_a_dependency_still_gives_the_importer_its_type() {
    let root = project(
        "initializer",
        "export declare const answer = 42;\nexport declare const label: 'hi';\n\
         export declare function f(x: number): string;\n",
        "import { answer, label, f } from 'dep';\n\
         const a: string = answer;\n\
         const b: number = label;\n\
         const c: string = f(1);\n",
    );
    // The first two are wrong, the third is right.
    assert_eq!(
        errors(&root),
        vec![
            ("main.ts".to_string(), Some(2322)),
            ("main.ts".to_string(), Some(2322)),
        ]
    );
}

#[test]
fn a_dependency_of_types_and_signatures_gives_the_importer_its_types() {
    let root = project(
        "signatures",
        "export interface Pair { first: string; second: number }\n\
         export declare const pair: Pair;\n\
         export declare function make(): Pair;\n\
         export type Id = string | number;\n",
        "import { pair, make, type Id } from 'dep';\n\
         const a: number = pair.first;\n\
         const b: boolean = make().second;\n\
         const i: Id = true;\n\
         const ok: string = pair.first;\n",
    );
    let found = errors(&root);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(
        found
            .iter()
            .all(|(file, code)| file == "main.ts" && *code == Some(2322))
    );
}
