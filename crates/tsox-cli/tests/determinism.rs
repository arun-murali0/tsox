#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::Path;
use std::process::Command;

// Problem: layers of files are checked in parallel, so a result that depended on which
// thread finished first would pass on a quiet machine and differ on a busy one.
// Picked: run the real binary on one project under different thread counts, several times
// each, and require the same exit code and the same bytes on stdout every time. The project
// has a diamond, a cycle, a barrel, a namespace, an enum, a class, a package declaration
// file and an ambient module, and a few real errors, so the output is not empty.
// Cost: this covers the parallel check only. The scheduler and the query layer will need
// their own versions of this test when they exist.

const FILES: &[(&str, &str)] = &[
    ("tsconfig.json", ""),
    (
        "diamond/d.ts",
        "export interface D { d: number }\nexport const bad: string = 1;\n",
    ),
    (
        "diamond/b.ts",
        "import { D } from './d';\nexport function fromB(x: D): number { return x.d }\n",
    ),
    (
        "diamond/c.ts",
        "import { D } from './d';\nexport function fromC(x: D): string { return x.d }\n",
    ),
    (
        "diamond/a.ts",
        "import { fromB } from './b';\nimport { fromC } from './c';\nconst n: string = fromB({ d: 1 });\nconst s: number = fromC({ d: 1 });\n",
    ),
    (
        "cycle/x.ts",
        "import { Y } from './y';\nexport interface X { y: Y }\nconst wrong: X = { y: { n: 'a' } };\n",
    ),
    (
        "cycle/y.ts",
        "import { X } from './x';\nexport interface Y { n: number }\nexport function peek(x: X): number { return x.y.n }\n",
    ),
    ("barrel/index.ts", "export * from './parts';\n"),
    (
        "barrel/parts.ts",
        "export enum Color { Red, Green }\nexport class K { n: number = 1 }\nexport namespace Geo { export const k: number = 1; export interface P { x: number } }\n",
    ),
    (
        "barrel/use.ts",
        "import { Color, K, Geo } from './index';\nconst a: string = Color.Red;\nconst b: string = new K().n;\nconst c: Geo.P = { x: 'q' };\nconst d: string = Geo.k;\n",
    ),
    (
        "node_modules/pkg/package.json",
        "{ \"name\": \"pkg\", \"version\": \"1.0.0\", \"types\": \"index.d.ts\" }",
    ),
    (
        "node_modules/pkg/index.d.ts",
        "export declare function twice(n: number): number;\n",
    ),
    (
        "pkgs/use.ts",
        "import { twice } from 'pkg';\nconst s: string = twice(1);\n",
    ),
    (
        "amb/types.d.ts",
        "declare module 'virtual' { export function make(): number; }\n",
    ),
    (
        "amb/use.ts",
        "import { make } from 'virtual';\nconst s: string = make();\n",
    ),
    (
        "missing/use.ts",
        "import { Nope } from './absent';\nimport { Also } from './diamond/d';\n",
    ),
];

fn write_project(root: &Path) {
    let _ = fs::remove_dir_all(root);
    for (name, text) in FILES {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
}

fn run(root: &Path, threads: usize) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_tsox"))
        .arg("--project")
        .arg(root.join("tsconfig.json"))
        .env("RAYON_NUM_THREADS", threads.to_string())
        .output()
        .expect("the binary runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
fn the_same_project_gives_the_same_bytes_at_any_thread_count() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("determinism-project");
    write_project(&root);

    let (code, baseline) = run(&root, 1);
    assert_eq!(code, Some(1), "the project has real errors: {baseline}");
    // Not vacuous: errors from several parts of the project are in the output.
    for needle in ["Checked ", "error"] {
        assert!(baseline.contains(needle), "missing {needle} in {baseline}");
    }
    assert!(baseline.lines().count() > 8, "{baseline}");

    for threads in [1, 2, 3, 8] {
        for round in 0..3 {
            let (code_again, again) = run(&root, threads);
            assert_eq!(
                code_again, code,
                "exit code, {threads} threads, round {round}"
            );
            assert_eq!(again, baseline, "stdout, {threads} threads, round {round}");
        }
    }
}

#[test]
fn the_order_files_are_written_in_does_not_change_the_output() {
    // The binary sorts what it finds, so creating the same files in the reverse order
    // must not change a byte.
    let forward = Path::new(env!("CARGO_TARGET_TMPDIR")).join("determinism-forward");
    let backward = Path::new(env!("CARGO_TARGET_TMPDIR")).join("determinism-backward");
    write_project(&forward);
    let _ = fs::remove_dir_all(&backward);
    for (name, text) in FILES.iter().rev() {
        let path = backward.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let (_, a) = run(&forward, 4);
    let (_, b) = run(&backward, 4);
    // Paths differ by the project folder name, so compare with it taken out.
    let a = a.replace(forward.to_str().unwrap(), "ROOT");
    let b = b.replace(backward.to_str().unwrap(), "ROOT");
    assert_eq!(a, b);
}
