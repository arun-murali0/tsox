#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// Problem: the command line checked every `.ts` file under the config's directory and
// ignored `include`, `exclude` and `files`; `--project tsconfig.json` given as a bare file
// name found no files, because its parent path is empty; and it never set the project
// root, so a missing package could not be reported as an error.
// Picked: the command line loads the config and checks the files it selects, treats an
// empty parent as the current directory, and gives the graph the root and the config.
// Now: the tests below run the real binary.

fn project(test: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-project-{test}"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    for (name, text) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    root
}

fn run(dir: &Path, project_arg: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tsox"))
        .current_dir(dir)
        .args(["--project", project_arg])
        .output()
        .expect("the binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn nothing_above_answers_bare_specifiers(root: &Path) -> bool {
    !root
        .ancestors()
        .any(|dir| dir.join("node_modules").join("@types").is_dir())
}

#[test]
fn a_bare_config_file_name_finds_the_files_in_the_current_directory() {
    let root = project(
        "bare-name",
        &[
            ("tsconfig.json", "{}"),
            ("a.ts", "export const a: number = 1;\n"),
        ],
    );
    let output = run(&root, "tsconfig.json");
    let text = stdout(&output);
    assert!(text.contains("Checked 1 file(s)"), "{text}");
    assert!(output.status.success(), "{text}");
}

#[test]
fn include_and_exclude_choose_which_files_are_checked() {
    // `src/bad.ts` has a type error; the config includes `src/good.ts` only through a
    // directory that excludes `bad.ts`, so the run is clean and checks one file.
    let root = project(
        "include-exclude",
        &[
            (
                "tsconfig.json",
                r#"{ "include": ["src"], "exclude": ["src/bad.ts"] }"#,
            ),
            ("src/good.ts", "export const g: number = 1;\n"),
            ("src/bad.ts", "export const b: number = 'text';\n"),
            ("outside.ts", "export const o: number = 'text';\n"),
        ],
    );
    let output = run(&root, "tsconfig.json");
    let text = stdout(&output);
    assert!(text.contains("Checked 1 file(s): 0 error(s)"), "{text}");
    assert!(output.status.success(), "{text}");
}

#[test]
fn a_type_error_in_a_selected_file_fails_the_run() {
    let root = project(
        "selected-error",
        &[
            ("tsconfig.json", r#"{ "include": ["src"] }"#),
            ("src/bad.ts", "export const b: number = 'text';\n"),
        ],
    );
    let output = run(&root, "tsconfig.json");
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
}

#[test]
fn a_path_alias_resolves_and_a_missing_package_is_an_error() {
    let root = project(
        "alias-and-missing",
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
    if !nothing_above_answers_bare_specifiers(&root) {
        return;
    }
    let output = run(&root, "tsconfig.json");
    let text = stdout(&output);
    assert!(text.contains("left-pad"), "{text}");
    assert!(
        !text.contains("@lib/util"),
        "the alias should resolve: {text}"
    );
    assert_eq!(output.status.code(), Some(1), "{text}");
}

#[test]
fn a_config_that_cannot_be_read_falls_back_to_every_file_with_a_warning() {
    let root = project(
        "bad-config",
        &[
            ("tsconfig.json", "{ \"include\": [ }"),
            ("a.ts", "export const a: number = 1;\n"),
        ],
    );
    let output = run(&root, "tsconfig.json");
    let text = stdout(&output);
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(errors.contains("cannot parse"), "{errors}");
    assert!(text.contains("Checked 1 file(s)"), "{text}");
}
