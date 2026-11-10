#![cfg(feature = "incremental")]

//! `IncrementalProject::type_at`: the type the checker gives the thing at an offset.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tsox::IncrementalProject;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("type-at-{}", NEXT.fetch_add(1, Ordering::SeqCst)));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let dir = Self(fs::canonicalize(&root).unwrap());
        fs::write(
            dir.0.join("tsconfig.json"),
            r#"{ "compilerOptions": { "strict": true, "module": "esnext", "moduleResolution": "bundler" }, "include": ["*.ts"] }"#,
        )
        .unwrap();
        for (name, text) in files {
            fs::write(dir.0.join(name), text).unwrap();
        }
        dir
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn project(&self) -> IncrementalProject {
        let mut project = IncrementalProject::open(&self.0.join("tsconfig.json")).unwrap();
        project.check().unwrap();
        project
    }
}

const LIB: &str = "export function twice(n: number): number { return n * 2; }\nexport interface Point { x: number; y: number }\nexport const origin: Point = { x: 0, y: 0 };\n";

const MAIN: &str = "import { twice, origin } from './lib';\nconst n = twice(2);\nlet s = 'x';\nconst arr = [1, 2, 3];\nconst o = { a: 1, b: 'two' };\nfunction add(a: number, b: number) { return a + b; }\nconst total = add(n, 1);\nconst f = (x: string) => x.length;\nconst px = origin.x;\n";

/// The type text at the first occurrence of `needle`, plus `into` bytes.
fn type_of(
    project: &IncrementalProject,
    path: &Path,
    text: &str,
    needle: &str,
    into: usize,
) -> Option<String> {
    let at = text.find(needle).expect("needle present") + into;
    project.type_at(path, at as u32).map(|found| found.text)
}

#[test]
fn declarations_and_uses_get_their_inferred_types() {
    let dir = Dir::new(&[("lib.ts", LIB), ("main.ts", MAIN)]);
    let project = dir.project();
    let main = dir.path("main.ts");
    let t = |needle: &str, into: usize| type_of(&project, &main, MAIN, needle, into);

    assert_eq!(
        t("n =", 0).as_deref(),
        Some("number"),
        "a const inferred from a call"
    );
    assert_eq!(
        t("s =", 0).as_deref(),
        Some("string"),
        "a let widens a literal"
    );
    assert_eq!(t("arr =", 0).as_deref(), Some("number[]"));
    assert_eq!(t("o =", 0).as_deref(), Some("{ a: number; b: string; }"));
    assert_eq!(t("total =", 0).as_deref(), Some("number"));
    assert_eq!(t("f =", 0).as_deref(), Some("(x: string) => number"));
    assert_eq!(t("px =", 0).as_deref(), Some("number"));
    // A function name at its declaration, and a parameter.
    assert_eq!(
        t("add(a", 0).as_deref(),
        Some("(a: number, b: number) => number")
    );
    assert_eq!(t("a: number", 0).as_deref(), Some("number"));
}

#[test]
fn a_use_gets_the_type_of_the_expression_it_is_in() {
    let dir = Dir::new(&[("lib.ts", LIB), ("main.ts", MAIN)]);
    let project = dir.project();
    let main = dir.path("main.ts");
    let t = |needle: &str, into: usize| type_of(&project, &main, MAIN, needle, into);

    // On the callee of a call: its own type, not the call's.
    assert_eq!(
        t("twice(2)", 0).as_deref(),
        Some("(n: number) => number"),
        "the callee"
    );
    assert_eq!(t("twice(2)", 6).as_deref(), Some("2"), "the argument");
    assert_eq!(t("origin.x", 0).as_deref(), Some("Point"), "on `origin`");
    assert_eq!(
        t("origin.x", "origin.".len()).as_deref(),
        Some("number"),
        "on `x`"
    );
    assert_eq!(t("x.length", 0).as_deref(), Some("string"));
    assert_eq!(t("x.length", 2).as_deref(), Some("number"));
}

#[test]
fn types_come_through_an_import_from_another_file() {
    let dir = Dir::new(&[("lib.ts", LIB), ("main.ts", MAIN)]);
    let project = dir.project();
    let main = dir.path("main.ts");
    let t = |needle: &str, into: usize| type_of(&project, &main, MAIN, needle, into);

    assert_eq!(
        t("twice, origin", 0).as_deref(),
        Some("(n: number) => number")
    );
    assert_eq!(t("origin }", 0).as_deref(), Some("Point"));
}

#[test]
fn the_innermost_expression_is_the_one_reported() {
    let text = "const v = (1 + 2) * 3;\n";
    let dir = Dir::new(&[("a.ts", text)]);
    let project = dir.project();
    let a = dir.path("a.ts");
    let found = project
        .type_at(&a, text.find("2)").unwrap() as u32)
        .unwrap();
    assert_eq!(&text[found.start as usize..found.end as usize], "2");
    let found = project
        .type_at(&a, text.find("* 3").unwrap() as u32 + 2)
        .unwrap();
    assert_eq!(&text[found.start as usize..found.end as usize], "3");
}

#[test]
fn an_unsaved_buffer_is_what_is_probed() {
    let dir = Dir::new(&[("a.ts", "const v = 1;\n")]);
    let mut project = dir.project();
    let a = dir.path("a.ts");
    let edited = "const v = 'now a string';\n";
    project.set_buffer(&a, edited);
    project.check().unwrap();
    // A `const` keeps the literal, as it does in TypeScript.
    assert_eq!(
        type_of(&project, &a, edited, "v =", 0).as_deref(),
        Some("\"now a string\"")
    );
    let widened = "let v = 'now a string';\n";
    project.set_buffer(&a, widened);
    project.check().unwrap();
    assert_eq!(
        type_of(&project, &a, widened, "v =", 0).as_deref(),
        Some("string")
    );
}

#[test]
fn a_dependency_changed_in_a_buffer_is_seen_by_the_file_that_imports_it() {
    let dir = Dir::new(&[
        ("lib.ts", "export const value = 1;\n"),
        (
            "main.ts",
            "import { value } from './lib';\nconst copy = value;\n",
        ),
    ]);
    let mut project = dir.project();
    let main = dir.path("main.ts");
    let text = "import { value } from './lib';\nconst copy = value;\n";
    assert_eq!(
        type_of(&project, &main, text, "copy", 0).as_deref(),
        Some("1")
    );
    project.set_buffer(&dir.path("lib.ts"), "export const value = 'text';\n");
    project.check().unwrap();
    assert_eq!(
        type_of(&project, &main, text, "copy", 0).as_deref(),
        Some("\"text\"")
    );
}

#[test]
fn nothing_typed_at_the_offset_gives_none() {
    let text = "// a comment\nconst v = 1;\n\n";
    let dir = Dir::new(&[("a.ts", text)]);
    let project = dir.project();
    let a = dir.path("a.ts");
    assert_eq!(type_of(&project, &a, text, "comment", 0), None);
    assert_eq!(type_of(&project, &a, text, "const", 0), None, "a keyword");
    assert_eq!(
        project.type_at(&a, text.len() as u32),
        None,
        "the end of the file"
    );
    assert_eq!(project.type_at(&a, 9999), None, "past the end");
}

#[test]
fn a_file_that_is_not_in_the_project_gives_none() {
    let dir = Dir::new(&[("a.ts", "const v = 1;\n")]);
    let project = dir.project();
    assert_eq!(project.type_at(&dir.path("elsewhere.ts"), 0), None);
    assert_eq!(project.type_at(Path::new("/no/such/file.ts"), 0), None);
}

#[test]
fn asking_before_any_check_does_not_panic() {
    let dir = Dir::new(&[("a.ts", "const v = 1;\n")]);
    let project = IncrementalProject::open(&dir.path("tsconfig.json")).unwrap();
    let _ = project.type_at(&dir.path("a.ts"), 6);
}

#[test]
fn probing_changes_nothing_a_check_reports() {
    let text = "const bad: number = 'x';\nconst ok = 1;\n";
    let dir = Dir::new(&[("a.ts", text)]);
    let mut project = dir.project();
    let a = dir.path("a.ts");
    let before = format!("{:?}", project.check().unwrap());
    for offset in 0..text.len() as u32 {
        let _ = project.type_at(&a, offset);
    }
    let after = project.check().unwrap();
    assert_eq!(format!("{after:?}"), before);
    assert_eq!(
        project.stats().units_checked,
        0,
        "the probes memoized nothing and made nothing stale"
    );
}

#[test]
fn files_in_a_circular_import_see_each_other() {
    let a =
        "import { fb } from './b';\nexport const va = 1;\nexport function fa() { return fb(); }\n";
    let b = "import { va } from './a';\nexport function fb() { return va; }\nconst local = va;\n";
    let dir = Dir::new(&[("a.ts", a), ("b.ts", b)]);
    let project = dir.project();
    assert_eq!(
        type_of(&project, &dir.path("b.ts"), b, "local", 0).as_deref(),
        Some("1")
    );
    assert_eq!(
        type_of(&project, &dir.path("a.ts"), a, "va =", 0).as_deref(),
        Some("1")
    );
}

#[test]
fn a_file_with_a_type_error_is_still_probed() {
    let text = "const bad: number = 'x';\nconst fine = [true];\n";
    let dir = Dir::new(&[("a.ts", text)]);
    let project = dir.project();
    let a = dir.path("a.ts");
    assert_eq!(
        type_of(&project, &a, text, "fine", 0).as_deref(),
        Some("boolean[]")
    );
    assert_eq!(
        type_of(&project, &a, text, "bad", 0).as_deref(),
        Some("number")
    );
}

#[test]
fn the_standard_library_types_are_known() {
    let text = "const top = Math.max(1, 2);\nconst parts = 'a,b'.split(',');\n";
    let dir = Dir::new(&[("a.ts", text)]);
    let project = dir.project();
    let a = dir.path("a.ts");
    assert_eq!(
        type_of(&project, &a, text, "top =", 0).as_deref(),
        Some("number")
    );
    assert_eq!(
        type_of(&project, &a, text, "parts", 0).as_deref(),
        Some("string[]")
    );
}

#[test]
fn a_type_the_checker_could_not_work_out_is_not_reported_as_one() {
    // The checker has no model of some library members yet; what it holds for them is its
    // internal error type, which must not reach a reader as if it were a type.
    let text = "const now = Date.now();\nconst ok = 1;\n";
    let dir = Dir::new(&[("a.ts", text)]);
    let project = dir.project();
    let a = dir.path("a.ts");
    let found = type_of(&project, &a, text, "now =", 0);
    assert_ne!(found.as_deref(), Some("error"));
    assert_eq!(type_of(&project, &a, text, "ok =", 0).as_deref(), Some("1"));
}

#[test]
fn a_method_call_and_a_constructor_call_give_the_type_of_what_is_called() {
    let text = "class Box { get(n: number): string { return String(n); } }\nconst box = new Box();\nconst got = box.get(1);\n";
    let dir = Dir::new(&[("a.ts", text)]);
    let project = dir.project();
    let a = dir.path("a.ts");
    let t = |needle: &str, into: usize| type_of(&project, &a, text, needle, into);
    assert_eq!(
        t("get(1)", 0).as_deref(),
        Some("(n: number) => string"),
        "the method being called"
    );
    assert_eq!(t("got =", 0).as_deref(), Some("string"));
    let constructor = t("Box();", 0);
    assert!(
        constructor.is_some(),
        "the class being constructed: {constructor:?}"
    );
}
