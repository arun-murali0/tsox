use std::fs;
use std::path::{Path, PathBuf};

// Problem: every generated fixture in complex_fixtures.rs is repetitive, so caches and the
// arena see the same shapes again and again and the numbers come out kinder than on code
// somebody actually wrote.
// Picked: a fixed slice of the TypeScript 6.0.3 corpus. The files are sorted by path and
// every `stride`-th one is taken, so the same checkout always gives the same slice.
// TSOX_CORPUS_STRIDE and TSOX_CORPUS_OFFSET are the same variables
// tests/corpus_aborts.rs reads, so one setting picks the same files for both.
// Cost: files are checked one at a time on their own (the flat mode of the binary), and a
// file that does not read as UTF-8 is skipped.

pub struct CorpusFile {
    pub name: String,
    pub source: String,
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/typescript-testcases/typescript-6.0.3")
}

fn ts_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            files.extend(ts_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "ts") {
            files.push(path);
        }
    }
    files
}

fn from_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

// `default_stride` is per caller: the callgrind run is far slower than a timed one, so it
// takes a smaller slice.
pub fn corpus_slice(default_stride: usize) -> Vec<CorpusFile> {
    let stride = from_env("TSOX_CORPUS_STRIDE", default_stride).max(1);
    let offset = from_env("TSOX_CORPUS_OFFSET", 0);
    let root = corpus_dir();
    let slice: Vec<CorpusFile> = ts_files(&root)
        .into_iter()
        .skip(offset)
        .step_by(stride)
        .filter_map(|path| {
            let source = fs::read_to_string(&path).ok()?;
            let name = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
            Some(CorpusFile { name, source })
        })
        .collect();
    assert!(
        !slice.is_empty(),
        "no .ts files under {}; is the corpus checked out?",
        root.display()
    );
    slice
}
