//! Repository tooling in Rust: `cargo xtask <command>`.
//!
//! * `layering`            fail when a crate depends on a crate at its own layer or above
//! * `owners [--check]`    write or check docs/architecture/diagnostic-owners.md
//! * `corpus <command>`    the tsc corpus: fetch by tag, run, verify against the map, build, update, status
//!
//! The commands only read the repository, call `git`, `cargo metadata` and `node`
//! (`scripts/ts-diag-tool/compare.js`, which is what talks to the real tsc), and write the map.

mod corpus;
mod layering;
mod owners;

use std::path::PathBuf;
use std::process::ExitCode;

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("layering") => layering::run(),
        Some("owners") => owners::run(args.iter().any(|a| a == "--check")),
        Some("corpus") => corpus::run(&args[1..]),
        _ => {
            eprintln!(
                "usage: cargo xtask layering | owners [--check] | corpus fetch|run|verify|build|update|status"
            );
            2
        }
    };
    ExitCode::from(code)
}
