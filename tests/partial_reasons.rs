#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;
use std::process::Command;

use tsox::{
    FileId, IncompleteReason, ModuleGraph, ModuleKey, ModuleResolver, UnitContext, Worker,
    check_project,
};

// Problem: a unit whose export list is partial was only a bool, so there was no way to say
// why, which is what decides what to build next, and no test could pin one case apart from
// the rest.
// Picked: every place the collector gives up names a reason, the reasons travel with the unit
// output and the file report, and the CLI prints them with --partial. These tests give each
// reason a source that produces it and nothing else.
// Cost: a reason is a fixed phrase, so it says which construct was met and not where.

use IncompleteReason as Why;

fn reasons_of(file: &str, source: &str) -> Vec<IncompleteReason> {
    let ctx = UnitContext::new(FileId::ROOT, file, source).with_module(ModuleKey::project(file));
    Worker::new().check_unit(&ctx).unwrap().incomplete_reasons
}

fn reasons(source: &str) -> Vec<IncompleteReason> {
    reasons_of("a.ts", source)
}

#[test]
fn a_complete_unit_has_no_reasons() {
    assert!(reasons("export const ok = 1;\nexport enum E { A }").is_empty());
    assert!(reasons("export default class {}").is_empty());
    assert!(reasons("export default function (a: number) { return a; }").is_empty());
}

#[test]
fn a_pattern_that_binds_nothing_is_named() {
    assert_eq!(
        reasons("export const {} = {};"),
        vec![Why::EmptyBindingPattern]
    );
}

#[test]
fn a_shorthand_ambient_module_is_named_even_though_the_file_lists_are_complete() {
    assert_eq!(
        reasons("declare module \"x\";"),
        vec![Why::ShorthandAmbientModule]
    );
}

#[test]
fn an_anonymous_default_function_with_no_body_or_return_type_is_named() {
    assert_eq!(
        reasons("export default function (a: number);"),
        vec![Why::AnonymousDefaultWithoutSignature]
    );
}

#[test]
fn export_equals_of_a_value_that_may_carry_properties_is_named() {
    assert_eq!(
        reasons("function pad(a: string): string { return a; }\nexport = pad;"),
        vec![Why::NonObjectExportAssignment]
    );
    assert_eq!(
        reasons("const n = 1;\nexport = n;"),
        vec![Why::NonObjectExportAssignment]
    );
}

#[test]
fn an_export_of_a_name_the_file_does_not_declare_is_named() {
    assert_eq!(reasons("export { nope };"), vec![Why::UnknownLocalExport]);
}

#[test]
fn export_star_from_an_unknown_module_is_named() {
    assert_eq!(
        reasons("export * from './missing';"),
        vec![Why::StarFromPartialModule]
    );
}

#[test]
fn export_star_as_namespace_from_an_unknown_module_is_named() {
    let found = reasons("export * as ns from './missing';");
    assert!(
        found.contains(&Why::NamespaceStarFromPartialModule),
        "{found:?}"
    );
}

#[test]
fn an_exported_signature_that_cannot_be_written_out_is_named() {
    assert_eq!(
        reasons("export function f(a: Missing): void {}"),
        vec![Why::UnrepresentableDeclaration]
    );
}

#[test]
fn an_exported_import_alias_is_an_unsupported_declaration() {
    assert_eq!(
        reasons("export import q = Foo.Bar;"),
        vec![Why::UnsupportedDeclaration]
    );
}

// tsc exports the unmarked declarations of a declaration file unless it has an export
// statement, so without one the list cannot be called complete; with one it can.
#[test]
fn a_declaration_file_without_an_export_statement_is_named() {
    let hidden = "declare interface Hidden { value: number }\n";
    assert_eq!(
        reasons_of(
            "types.d.ts",
            &format!("{hidden}export declare function pad(s: string): string;")
        ),
        vec![Why::DeclarationFileWithoutExportStatement]
    );
    assert!(
        reasons_of(
            "types.d.ts",
            &format!("{hidden}export declare function pad(s: string): string;\nexport {{}};")
        )
        .is_empty()
    );
}

#[test]
fn a_panic_in_the_checker_is_named() {
    // The worker turns a panic into a crashed output; the unit tests in src/unit.rs make one,
    // and this pins what the output says about why.
    assert_eq!(
        Why::CheckerCrashed.describe(),
        "the checker crashed on the file"
    );
}

#[test]
fn every_reason_has_its_own_phrase() {
    let all = [
        Why::CheckerCrashed,
        Why::UnsupportedDeclaration,
        Why::EmptyBindingPattern,
        Why::ShorthandAmbientModule,
        Why::AnonymousDefaultWithoutSignature,
        Why::UnsupportedDefaultExport,
        Why::NonObjectExportAssignment,
        Why::UnknownLocalExport,
        Why::UnresolvedReExport,
        Why::DeclarationFileWithoutExportStatement,
        Why::UnrepresentableDeclaration,
        Why::StarFromPartialModule,
        Why::NamespaceStarFromPartialModule,
    ];
    let mut phrases: Vec<&str> = all.iter().map(|reason| reason.describe()).collect();
    assert!(phrases.iter().all(|phrase| !phrase.is_empty()));
    phrases.sort_unstable();
    phrases.dedup();
    assert_eq!(phrases.len(), all.len(), "two reasons share a phrase");
}

fn write(test: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("partial-reasons-{test}"));
    let _ = fs::remove_dir_all(&root);
    for (name, source) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, source).unwrap();
    }
    root
}

// Through the project: a re-export of a name the other file does not have makes the file
// partial, and the file report says so while the files around it stay complete.
#[test]
fn a_project_report_carries_the_reasons_of_each_file() {
    let root = write(
        "project",
        &[
            ("b.ts", "export const y = 1;"),
            ("a.ts", "import { x } from './b';\nexport { x };"),
            ("c.ts", "import { x } from './a';\nconst n: number = x;"),
        ],
    );
    let paths = ["b.ts", "a.ts", "c.ts"]
        .map(|name| root.join(name))
        .to_vec();
    let graph = ModuleGraph::build(&paths, &ModuleResolver::new())
        .expect("the project builds")
        .with_root(root);
    let report = check_project(&graph);
    let reasons_for = |name: &str| -> Vec<IncompleteReason> {
        report
            .files
            .iter()
            .find(|file| {
                graph
                    .files()
                    .path(file.file_id)
                    .is_some_and(|path| path.ends_with(name))
            })
            .map(|file| file.incomplete_reasons.clone())
            .unwrap()
    };
    assert!(reasons_for("b.ts").is_empty());
    assert!(!reasons_for("a.ts").is_empty(), "{:?}", reasons_for("a.ts"));
    assert!(reasons_for("c.ts").is_empty());
}

#[test]
fn the_cli_prints_the_partial_files_and_why_with_the_partial_flag() {
    let root = write(
        "cli",
        &[
            ("tsconfig.json", "{}"),
            ("a.ts", "export { nope };"),
            ("b.ts", "export const fine = 1;"),
        ],
    );
    let run = |extra: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_tsox"))
            .arg("--project")
            .arg(root.join("tsconfig.json"))
            .args(extra)
            .output()
            .expect("the binary runs");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let with = run(&["--partial"]);
    assert!(
        with.contains("a.ts: exports partial: an export names something not declared in the file"),
        "{with}"
    );
    assert!(!with.contains("b.ts: exports partial"), "{with}");
    assert!(!run(&[]).contains("exports partial"));
}
