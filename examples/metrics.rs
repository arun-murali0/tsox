// Prints the counters a CheckSession keeps after a check, for one file, checked twice so
// the second run shows the reused arena. Run it with:
//   cargo run --example metrics -- path/to/file.ts
use std::{env, fs, process};

use tsox::{FileId, TypeChecker};

fn main() {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: metrics <file.ts>");
        process::exit(2);
    };
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("cannot read {path}: {error}");
            process::exit(2);
        }
    };

    let mut session = TypeChecker::new().session(FileId::ROOT);
    for run in 1..=2 {
        let result = match session.check_source(&source, &path) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("{path}: {error:?}");
                process::exit(2);
            }
        };
        let Some(metrics) = session.last_metrics() else {
            continue;
        };
        println!("run {run}: {} diagnostics", result.diagnostics.len());
        println!("  arena     {:?}", metrics.arena);
        println!("  relations {:?}", metrics.queries);
        println!("  namespace {:?}", metrics.namespace);
    }
}
