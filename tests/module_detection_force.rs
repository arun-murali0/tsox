use tsox::{CheckOptions, FileId, Severity, UnitContext, Worker};

// `moduleDetection: "force"`: a file with no import or export is still a module. Compared with
// tsc 6.0.3 (`declare global` in such a file is fine, and its top-level names are not global).

fn error_messages(source: &str, file: &str, force: bool) -> Vec<String> {
    let options = CheckOptions {
        module_detection_force: force,
        ..CheckOptions::default()
    };
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, file, source).with_options(options);
    let outcome = worker.check_unit(&ctx).expect("the unit checks");
    outcome
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect()
}

const GLOBAL_BLOCK: &str =
    "declare global { interface Foo { z: number } }\nvar fo: Foo = { z: 1 };\n";

#[test]
fn a_file_with_no_import_or_export_is_a_script_by_default() {
    let found = error_messages(GLOBAL_BLOCK, "t.ts", false);
    assert!(found.iter().any(|m| m.contains("global")), "{found:?}");
}

#[test]
fn forced_detection_makes_it_a_module() {
    let found = error_messages(GLOBAL_BLOCK, "t.ts", true);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_declaration_file_stays_a_script_when_detection_is_forced() {
    let found = error_messages("declare global { interface Foo {} }\n", "t.d.ts", true);
    assert!(found.iter().any(|m| m.contains("global")), "{found:?}");
}
