#![cfg(feature = "incremental")]

// Step 0 of the architecture plan: how many units run again after each kind of edit. The
// numbers are printed (`cargo test --all-features --test rerun_accounting -- --nocapture`) so
// a change that is meant to cut reruns, such as option reads in the typed read log, has a
// before and an after to compare. The assertions are only the facts that must always hold.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tsox::IncrementalProject;

static NEXT: AtomicUsize = AtomicUsize::new(0);
const CHAIN: usize = 12;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "rerun-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let dir = Self(fs::canonicalize(&root).unwrap());
        dir.write("tsconfig.json", &config(false));
        dir.write("m0.ts", &module(0, 1));
        for i in 1..CHAIN {
            dir.write(&format!("m{i}.ts"), &module(i, 1));
        }
        // A script: no import or export, so its names are global.
        dir.write("script.ts", "interface Marker { tag: number }\n");
        dir
    }

    fn write(&self, file: &str, text: &str) {
        fs::write(self.0.join(file), text).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        fs::OpenOptions::new()
            .write(true)
            .open(self.0.join(file))
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(later))
            .unwrap();
    }
}

fn config(unchecked_index: bool) -> String {
    format!(
        r#"{{ "compilerOptions": {{ "strict": true, "noUncheckedIndexedAccess": {unchecked_index}, "module": "esnext", "moduleResolution": "bundler" }}, "include": ["*.ts"] }}"#
    )
}

// Module `i` imports `i - 1` (module 0 imports nothing) and exports a function whose body
// returns `value`, with an annotated signature.
fn module(i: usize, value: usize) -> String {
    if i == 0 {
        return format!("export function f0(): number {{ return {value}; }}\n");
    }
    format!(
        "import {{ f{p} }} from './m{p}';\nexport function f{i}(): number {{ return f{p}() + {value}; }}\n",
        p = i - 1
    )
}

fn runs(project: &mut IncrementalProject) -> usize {
    project
        .take_log()
        .iter()
        .filter(|entry| entry.starts_with("scc_run:"))
        .count()
}

#[test]
fn units_rerun_after_each_kind_of_edit() {
    let dir = Dir::new();
    let mut project = IncrementalProject::open(&dir.0.join("tsconfig.json")).unwrap();
    project.check().unwrap();
    let cold = runs(&mut project);
    assert_eq!(cold, CHAIN + 1, "every unit runs on the first check");

    // An unchanged save.
    dir.write("m5.ts", &module(5, 1));
    project.check().unwrap();
    let unchanged = runs(&mut project);
    assert_eq!(unchanged, 0, "an unchanged save runs nothing");

    // A body edit: the body of m5 changes, its signature does not.
    dir.write("m5.ts", &module(5, 2));
    project.check().unwrap();
    let body = runs(&mut project);

    // A script edit: a global interface changes, nobody uses it.
    dir.write("script.ts", "interface Marker { tag: string }\n");
    project.check().unwrap();
    let script = runs(&mut project);

    // An option change no unit reads: only the index-access option flips.
    dir.write("tsconfig.json", &config(true));
    project.check().unwrap();
    let option = runs(&mut project);

    println!("units: {}", CHAIN + 1);
    println!("first check      : {cold}");
    println!("unchanged save   : {unchanged}");
    println!("one body edit    : {body}");
    println!("one script edit  : {script}");
    println!("one option change: {option}");

    assert!(body >= 1 && body <= CHAIN + 1);
    assert!(script <= CHAIN + 1);
    assert_eq!(option, 0, "no unit read the option, so none runs again");
}

// An option change reaches exactly the units that read the option, and the answer after it is
// the one a cold check gives under the new options.
#[test]
fn an_option_change_reruns_only_the_units_that_read_it() {
    let dir = Dir::new();
    // `reader` indexes an array, which reads `noUncheckedIndexedAccess`; the chain does not.
    dir.write(
        "reader.ts",
        "const a: number[] = [];\nexport const x: number = a[0];\n",
    );
    let mut project = IncrementalProject::open(&dir.0.join("tsconfig.json")).unwrap();
    project.check().unwrap();
    let cold = runs(&mut project);
    assert_eq!(cold, CHAIN + 2);

    dir.write("tsconfig.json", &config(true));
    project.check().unwrap();
    let after = runs(&mut project);
    println!("option change with one reader: {after}");
    assert_eq!(after, 1, "only the unit that reads the option runs again");

    let reader = dir.0.join("reader.ts");
    let warm = format!("{:?}", project.file_report(&reader).unwrap().outcome);
    let mut fresh = IncrementalProject::open(&dir.0.join("tsconfig.json")).unwrap();
    fresh.check().unwrap();
    let cold_report = format!("{:?}", fresh.file_report(&reader).unwrap().outcome);
    assert_eq!(warm, cold_report, "an incremental result equals a cold one");
    assert!(
        warm.contains("number | undefined"),
        "the reader now reports number | undefined: {warm}"
    );
}
