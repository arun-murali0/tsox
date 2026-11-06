//! The opt-in library cache never changes what a check reports: a cold run writes entries, a
//! warm run reads them back, and both report what a run without the cache reports. A damaged
//! entry is checked again and written over. The cache is process-wide, so this is the only
//! test in its file.
#![cfg(all(feature = "module-resolution", feature = "snapshot"))]

use std::fs;
use std::path::Path;

use tsox::{
    DiskLibraryUnitCache, FileOutcome, ModuleGraph, ModuleResolver, ProjectConfig, Severity,
    check_project, install_library_unit_cache,
};

fn codes(root: &Path) -> Vec<(String, Option<u32>)> {
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
        .with_root(root)
        .with_config(&config);
    let report = check_project(&graph);
    let mut out = Vec::new();
    for file in &report.files {
        let name = graph
            .files()
            .path(file.file_id)
            .unwrap()
            .display()
            .to_string();
        match &file.outcome {
            FileOutcome::Checked(diagnostics) => {
                for d in diagnostics {
                    if d.severity == Severity::Error {
                        out.push((name.clone(), d.code.tsc_code()));
                    }
                }
            }
            other => panic!("{name}: {other:?}"),
        }
    }
    out.sort();
    out
}

#[test]
fn a_cold_and_a_warm_run_report_what_an_uncached_run_reports() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("library-unit-cache");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"target":"es2020","lib":["es2020"]},"files":["main.ts"]}"#,
    )
    .unwrap();
    fs::write(
        root.join("main.ts"),
        "const a: number[] = [1, 2];\nconst s: string = a.map(x => x + 1);\nconst p: Promise<number> = Promise.resolve(1);\n",
    )
    .unwrap();

    // Before the cache is installed.
    let uncached = codes(&root);
    assert!(!uncached.is_empty(), "the fixture must report something");

    let dir = root.join("cache");
    assert!(install_library_unit_cache(Box::new(
        DiskLibraryUnitCache::new(&dir).unwrap()
    )));
    let entries = || -> Vec<_> {
        fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "unit"))
            .collect()
    };
    assert!(entries().is_empty());

    assert_eq!(codes(&root), uncached, "a cold run");
    let written = entries();
    assert!(
        !written.is_empty(),
        "a cold run stores the library units that read nothing"
    );

    assert_eq!(codes(&root), uncached, "a warm run");
    assert_eq!(
        entries().len(),
        written.len(),
        "a warm run stores nothing new"
    );

    // A damaged entry, and a truncated one, are checked again and written over.
    fs::write(&written[0], b"not a snapshot").unwrap();
    let truncated = fs::read(&written[1]).unwrap();
    fs::write(&written[1], &truncated[..truncated.len() / 2]).unwrap();
    assert_eq!(codes(&root), uncached, "after damage");
    assert_ne!(fs::read(&written[0]).unwrap(), b"not a snapshot");
    assert_eq!(fs::read(&written[1]).unwrap(), truncated);
}
