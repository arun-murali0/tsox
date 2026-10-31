//! Times the query layer against the full driver on a project on disk.
//!
//! `cargo run --release --features incremental --example incremental_baseline -- <dir> <file> <body|signature>`
//! `<dir>` holds a tsconfig.json; `<file>` (relative to it) is edited between checks.

use std::path::PathBuf;
use std::time::Instant;

use tsox::{IncrementalProject, ModuleGraph, ModuleResolver, ProjectConfig, check_project};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&args[1]).canonicalize().unwrap();
    let file = dir.join(&args[2]);
    let kind = args.get(3).map_or("body", String::as_str);

    let begin = Instant::now();
    let mut project = IncrementalProject::open(&dir.join("tsconfig.json")).unwrap();
    println!("open (graph, inputs, headers):  {:?}", begin.elapsed());
    let begin = Instant::now();
    project.check().unwrap();
    println!(
        "first check (all units):        {:?}  {:?}",
        begin.elapsed(),
        project.stats()
    );
    let begin = Instant::now();
    project.refresh().unwrap();
    println!("refresh alone (stat 1,000 files): {:?}", begin.elapsed());
    let begin = Instant::now();
    project.check().unwrap();
    println!(
        "second check (nothing changed): {:?}  {:?}",
        begin.elapsed(),
        project.stats()
    );

    let original = std::fs::read_to_string(&file).unwrap();
    let edited = match kind {
        "signature" => format!("{original}\nexport interface Added{{ n: number }}\n"),
        "comment" => format!("{original}\n// edit\n"),
        _ => original.replacen("\"a\"", "\"b\"", 1),
    };
    std::fs::write(&file, &edited).unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    let handle = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
    handle
        .set_times(std::fs::FileTimes::new().set_modified(later))
        .unwrap();
    let begin = Instant::now();
    project.check().unwrap();
    println!(
        "after a {kind} edit:            {:?}  {:?}",
        begin.elapsed(),
        project.stats()
    );

    let config = ProjectConfig::load(dir.join("tsconfig.json")).unwrap();
    let resolver = ModuleResolver::with_tsconfig(config.config_file());
    let entries = config.project_entries(&resolver);
    let begin = Instant::now();
    let graph = ModuleGraph::build(&entries, &resolver)
        .unwrap()
        .with_root(config.root().to_path_buf())
        .with_config(&config);
    check_project(&graph);
    println!("full driver, same project:      {:?}", begin.elapsed());
    std::fs::write(&file, original).unwrap();
}
