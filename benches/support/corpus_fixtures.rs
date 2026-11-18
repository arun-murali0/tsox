use std::fs;
use std::path::{Path, PathBuf};

// Problem: every generated fixture in complex_fixtures.rs is repetitive, so caches and the
// arena see the same shapes again and again and the numbers come out kinder than on code
// somebody actually wrote.
// Picked: files of TypeScript's own test suite, fetched by tag (`cargo xtask corpus fetch`,
// into .corpus/ which git ignores). Three ways to choose files, all deterministic for one tag:
//   - a slice: the files sorted by path, every `stride`-th one (TSOX_CORPUS_STRIDE, TSOX_CORPUS_OFFSET;
//     the same variables tests/corpus_aborts.rs reads, so one setting picks the same files for both);
//   - a class, chosen by what the recorded map (tests/corpus/typescript-<tag>/) says about the file:
//     `clean` files draw no diagnostic from tsc or tsox, `errors` files draw the same ones from both.
//     These two time the quiet path and the diagnostic path apart, which a plain slice mixes;
//   - a whole part (TSOX_CORPUS_PART, default part-01), the unit the map and CI work in.
// Settings: TSOX_CORPUS_DIR (default <repo>/.corpus), TSOX_CORPUS_TAG (default v6.0.3),
// TSOX_CORPUS_LIMIT (files in a class, default per caller), TSOX_CORPUS_FETCH=1 fetches when the corpus is missing.
// Cost: files are checked one at a time on their own (the flat mode of the binary), and a
// file that does not read as UTF-8 is skipped.

pub struct CorpusFile {
    pub name: String,
    pub source: String,
}

pub const DEFAULT_TAG: &str = "v6.0.3";

fn tag() -> String {
    std::env::var("TSOX_CORPUS_TAG").unwrap_or_else(|_| DEFAULT_TAG.to_string())
}

fn tag_dir(tag: &str) -> String {
    format!("typescript-{}", tag.trim_start_matches('v'))
}

fn corpus_base() -> PathBuf {
    std::env::var_os("TSOX_CORPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".corpus"))
}

fn corpus_dir() -> PathBuf {
    let dir = corpus_base().join(tag_dir(&tag()));
    if !dir.join(".complete").exists() && std::env::var_os("TSOX_CORPUS_FETCH").is_some() {
        let status = std::process::Command::new("cargo")
            .args(["xtask", "corpus", "fetch", "--tag", &tag(), "--dest"])
            .arg(corpus_base())
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status();
        assert!(
            status.is_ok_and(|s| s.success()),
            "cargo xtask corpus fetch failed"
        );
    }
    dir
}

fn map_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(tag_dir(&tag()))
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

fn read(root: &Path, path: &Path) -> Option<CorpusFile> {
    let source = fs::read_to_string(path).ok()?;
    let name = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    Some(CorpusFile { name, source })
}

fn missing(root: &Path) -> String {
    format!(
        "no .ts files under {}; run `cargo xtask corpus fetch` (or set TSOX_CORPUS_FETCH=1)",
        root.display()
    )
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
        .filter_map(|path| read(&root, &path))
        .collect();
    assert!(!slice.is_empty(), "{}", missing(&root));
    slice
}

/// What the recorded map says about a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// tsc and tsox both report nothing: the quiet path of the checker.
    Clean,
    /// tsc and tsox report the same diagnostics: the path that builds and prints them.
    Errors,
}

// The map keeps one entry per line: `"<file>.ts": {"v":"MATCH","lines":[...]}`. Reading it
// takes no JSON parser: the file name and the verdict are at fixed places on the line.
fn class_names(class: Class) -> Vec<(String, String)> {
    let mut parts: Vec<PathBuf> = fs::read_dir(map_dir())
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    parts.retain(|p| p.extension().is_some_and(|e| e == "json"));
    parts.sort();
    let mut found = Vec::new();
    for path in parts {
        let part = path.file_stem().unwrap().to_string_lossy().into_owned();
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines() {
            let Some(rest) = line.strip_prefix('"') else {
                continue;
            };
            let Some((name, entry)) = rest.split_once("\": ") else {
                continue;
            };
            let matched = entry.starts_with("{\"v\":\"MATCH\"");
            let quiet = entry.contains("\"lines\":[]");
            if matched && (quiet == (class == Class::Clean)) {
                found.push((part.clone(), name.to_string()));
            }
        }
    }
    found
}

// Up to `default_limit` files of one class, spread evenly over the corpus.
pub fn corpus_class(class: Class, default_limit: usize) -> Vec<CorpusFile> {
    let limit = from_env("TSOX_CORPUS_LIMIT", default_limit).max(1);
    let root = corpus_dir();
    let names = class_names(class);
    let step = (names.len() / limit).max(1);
    let files: Vec<CorpusFile> = names
        .iter()
        .step_by(step)
        .take(limit)
        .filter_map(|(part, name)| read(&root, &root.join(part).join(name)))
        .collect();
    assert!(
        !files.is_empty(),
        "no {class:?} files from the map under {}: {}",
        map_dir().display(),
        missing(&root)
    );
    files
}

// One whole part of the corpus, the unit the map and CI work in.
#[allow(dead_code)]
pub fn corpus_part() -> Vec<CorpusFile> {
    let part = std::env::var("TSOX_CORPUS_PART").unwrap_or_else(|_| "part-01".to_string());
    let root = corpus_dir();
    let files: Vec<CorpusFile> = ts_files(&root.join(&part))
        .into_iter()
        .filter_map(|path| read(&root, &path))
        .collect();
    assert!(!files.is_empty(), "{}", missing(&root.join(part)));
    files
}
