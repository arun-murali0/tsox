#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;

use tsox::{FileOutcome, ModuleGraph, ModuleResolver, Severity, check_project};

// `class C<T> extends Array<T>`: the instance is an array too, so it has the array's members and
// is assignable to `T[]` and `ReadonlyArray<T>`. Run as a project, since a single file checked
// on its own cannot resolve the base class `Array`.

fn errors(test: &str, source: &str) -> Vec<String> {
    use tsox::ProjectConfig;
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("array-subclass-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"target":"es2020","lib":["es2020"],"moduleResolution":"bundler","module":"esnext"},"files":["a.ts"]}"#,
    )
    .unwrap();
    fs::write(root.join("a.ts"), source).unwrap();
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
        .with_root(&root)
        .with_config(&config);
    let report = check_project(&graph);
    let mut lines = Vec::new();
    for file in &report.files {
        let name = graph
            .files()
            .path(file.file_id)
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if name != "a.ts" {
            continue;
        }
        if let FileOutcome::Checked(diagnostics) = &file.outcome {
            for d in diagnostics {
                if d.severity == Severity::Error {
                    lines.push(format!("{}:{:?}", d.start, d.code.tsc_code()));
                }
            }
        }
    }
    lines.sort();
    lines
}

const CLASSES: &str = "class A { a = 1 }\nclass B extends A { b = 2 }\n\
    class C<T> extends Array<T> { c = 0 }\n\
    declare var ara: A[];\ndeclare var cra: C<A>;\ndeclare var crb: C<B>;\n\
    declare var rra: ReadonlyArray<A>;\ndeclare var rrb: ReadonlyArray<B>;\n";

#[test]
fn an_array_subclass_is_assignable_to_arrays_of_its_element_supertypes() {
    let source = format!(
        "{CLASSES}rra = cra;\nrra = crb;\nara = crb;\n\
         export const n: number = cra.length + cra.map((x) => x.a).length;\n\
         export {{ rra, ara, rrb }};\n"
    );
    assert_eq!(errors("ok", &source), Vec::<String>::new());
}

#[test]
fn an_array_subclass_is_not_assignable_to_arrays_of_unrelated_elements() {
    let source = format!("{CLASSES}rrb = cra;\nexport {{ rrb }};\n");
    let found = errors("bad", &source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].ends_with("Some(2322)"), "{found:?}");
}
