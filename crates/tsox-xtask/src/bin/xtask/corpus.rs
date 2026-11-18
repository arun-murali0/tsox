//! The tsc conformance corpus: fetched by tag, recorded as a map, checked against the map.
//!
//! The corpus is TypeScript's own test suite (microsoft/TypeScript, tests/cases/{compiler,conformance}
//! at a release tag). It is not stored in this repository. The result of comparing tsox with tsc over
//! it is: tests/corpus/typescript-<tag>/part-NN.json, one entry per file with the verdict and, per
//! line, what tsc and tsox said. A change is checked by running tsox over the corpus and comparing
//! with the map; tsc is not run again, because its answers are in the map.
//!
//!   cargo xtask corpus fetch  [--tag v6.0.3] [--dest .corpus]   download the suite, split into parts of 500
//!   cargo xtask corpus run    [--tag ..] [--parts 01,02]        run tsox over the parts against the map
//!   cargo xtask corpus verify [REPORT_DIR]                      compare run reports with the map
//!   cargo xtask corpus build  [REPORT_DIR]                      (re)write the map from reports made against real tsc
//!   cargo xtask corpus update [REPORT_DIR]                      accept the changes in a run
//!   cargo xtask corpus status [--readme]                        numbers from the map; --readme rewrites the README block
//!
//! `verify` exits 0 when map and run agree, 1 when something got worse, 3 when there are only
//! improvements or neutral changes (the map is stale: run `update` and commit it).

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const DEFAULT_TAG: &str = "v6.0.3";
const PART_SIZE: usize = 500;
const REPO: &str = "https://github.com/microsoft/TypeScript";

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct Line {
    #[serde(default)]
    line: u64,
    result: String,
    #[serde(default)]
    tsox: Vec<String>,
    #[serde(default)]
    tsc: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct Entry {
    v: String,
    lines: Vec<Line>,
}

#[derive(Deserialize)]
struct FileReport {
    file: String,
    verdict: String,
    #[serde(default)]
    lines: Vec<Line>,
}

#[derive(Deserialize)]
struct Report {
    files: Vec<FileReport>,
}

type Part = BTreeMap<String, Entry>;

struct Opts {
    tag: String,
    dest: PathBuf,
    reports: Option<PathBuf>,
    parts: Vec<String>,
    positional: Option<PathBuf>,
    readme: bool,
}

fn parse(args: &[String]) -> Opts {
    let root = crate::root();
    let mut o = Opts {
        tag: DEFAULT_TAG.into(),
        dest: root.join(".corpus"),
        reports: None,
        parts: vec![],
        positional: None,
        readme: false,
    };
    let mut i = 0;
    while i < args.len() {
        let next = |i: usize| args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--tag" => {
                o.tag = next(i);
                i += 1
            }
            "--dest" => {
                o.dest = PathBuf::from(next(i));
                i += 1
            }
            "--reports" => {
                o.reports = Some(PathBuf::from(next(i)));
                i += 1
            }
            "--parts" => {
                o.parts = next(i)
                    .split(',')
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect();
                i += 1
            }
            "--readme" => o.readme = true,
            other if !other.starts_with("--") => o.positional = Some(PathBuf::from(other)),
            _ => {}
        }
        i += 1;
    }
    o
}

fn tag_dir(tag: &str) -> &str {
    tag.trim_start_matches('v')
}
fn map_dir(tag: &str) -> PathBuf {
    crate::root()
        .join("tests/corpus")
        .join(format!("typescript-{}", tag_dir(tag)))
}
fn corpus_dir(o: &Opts) -> PathBuf {
    o.dest.join(format!("typescript-{}", tag_dir(&o.tag)))
}
fn reports_dir(o: &Opts) -> PathBuf {
    o.positional
        .clone()
        .or_else(|| o.reports.clone())
        .unwrap_or_else(|| o.dest.join("reports"))
}

// ---- fetch -------------------------------------------------------------------------------------

fn crashers() -> Vec<String> {
    fs::read_to_string(crate::root().join("tests/corpus/known-crashers.txt"))
        .map(|t| {
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out)
        } else {
            out.push(p)
        }
    }
}

/// UTF-8 with undecodable bytes replaced (a few cases are UTF-16) and every line ending turned into
/// LF. This is how the map was made, and it keeps the corpus the same on every platform.
fn normalised(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn fetch(o: &Opts) -> Result<PathBuf, String> {
    let out = corpus_dir(o);
    let marker = out.join(".complete");
    if marker.exists() {
        println!("{} is already fetched.", out.display());
        return Ok(out);
    }
    let work = o.dest.join(format!("_clone-{}", o.tag));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&o.dest).map_err(|e| e.to_string())?;
    let git = |args: &[&str]| -> Result<(), String> {
        let ok = Command::new("git")
            .args(args)
            .status()
            .map_err(|e| format!("git: {e}"))?
            .success();
        if ok {
            Ok(())
        } else {
            Err(format!("git {} failed", args.join(" ")))
        }
    };
    git(&[
        "clone",
        "--quiet",
        "--depth",
        "1",
        "--filter=blob:none",
        "--sparse",
        "--branch",
        &o.tag,
        REPO,
        &work.to_string_lossy(),
    ])?;
    git(&[
        "-C",
        &work.to_string_lossy(),
        "sparse-checkout",
        "set",
        "tests/cases/compiler",
        "tests/cases/conformance",
    ])?;

    // A case with more than one `// @filename:` is a multi-file project, which this corpus leaves out.
    let directive = Regex::new(r"(?im)^\s*//\s*@filename\s*:").unwrap();
    let cases = work.join("tests/cases");
    let mut chosen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for kind in ["compiler", "conformance"] {
        let mut files = Vec::new();
        walk(&cases.join(kind), &mut files);
        for path in files {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.ends_with(".ts") || name.ends_with(".d.ts") {
                continue;
            }
            let text =
                String::from_utf8_lossy(&fs::read(&path).map_err(|e| e.to_string())?).into_owned();
            if directive.find_iter(&text).count() > 1 {
                continue;
            }
            let rel = path.strip_prefix(&cases).unwrap();
            let joined: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            chosen.insert(joined.join("__"), path);
        }
    }
    for name in crashers() {
        chosen.remove(&name);
    }
    let _ = fs::remove_dir_all(&out);
    let mut count = 0;
    for (i, (name, path)) in chosen.iter().enumerate() {
        let part = out.join(format!("part-{:02}", i / PART_SIZE + 1));
        fs::create_dir_all(&part).map_err(|e| e.to_string())?;
        let text = normalised(&fs::read(path).map_err(|e| e.to_string())?);
        fs::write(part.join(name), text).map_err(|e| e.to_string())?;
        count = i + 1;
    }
    let _ = fs::remove_dir_all(&work);
    fs::write(&marker, format!("{} {} files\n", o.tag, count)).map_err(|e| e.to_string())?;
    println!("fetched {count} files of {} into {}", o.tag, out.display());
    Ok(out)
}

// ---- the map -----------------------------------------------------------------------------------

fn part_names(tag: &str) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(map_dir(tag))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".json")
                .map(String::from)
        })
        .filter(|n| n.starts_with("part-"))
        .collect();
    v.sort();
    v
}

fn read_part(tag: &str, part: &str) -> Result<Part, String> {
    let path = map_dir(tag).join(format!("{part}.json"));
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// One entry per line, sorted by file name, so a change shows up as a small diff.
fn write_part(tag: &str, part: &str, entries: &Part) -> Result<(), String> {
    fs::create_dir_all(map_dir(tag)).map_err(|e| e.to_string())?;
    let mut text = String::from("{\n");
    let last = entries.len();
    for (i, (name, entry)) in entries.iter().enumerate() {
        let comma = if i + 1 < last { "," } else { "" };
        text += &format!(
            "{}: {}{comma}\n",
            serde_json::to_string(name).unwrap(),
            serde_json::to_string(entry).unwrap()
        );
    }
    text += "}\n";
    fs::write(map_dir(tag).join(format!("{part}.json")), text).map_err(|e| e.to_string())
}

fn load_reports(dir: &Path) -> Result<BTreeMap<String, Part>, String> {
    let mut out = BTreeMap::new();
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy();
            n.starts_with("part-") && n.ends_with(".json")
        })
        .collect();
    paths.sort();
    for path in paths {
        let part = path.file_stem().unwrap().to_string_lossy().into_owned();
        let report: Report = serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut entries = Part::new();
        for f in report.files {
            let name = Path::new(&f.file)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let mut lines = f.lines;
            lines.sort_by_key(|l| l.line);
            entries.insert(
                name,
                Entry {
                    v: f.verdict,
                    lines,
                },
            );
        }
        out.insert(part, entries);
    }
    if out.is_empty() {
        return Err(format!("no part-NN.json reports in {}", dir.display()));
    }
    Ok(out)
}

/// Writes the map from reports. A file a report does not list had nothing on either side.
fn build(o: &Opts) -> Result<u8, String> {
    for (part, found) in load_reports(&reports_dir(o))? {
        let mut names: Vec<String> = found.keys().cloned().collect();
        let folder = corpus_dir(o).join(&part);
        if folder.is_dir() {
            names.extend(
                fs::read_dir(&folder)
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".ts")),
            );
        } else if map_dir(&o.tag).join(format!("{part}.json")).exists() {
            names.extend(read_part(&o.tag, &part)?.into_keys());
        }
        let entries: Part = names
            .into_iter()
            .map(|n| {
                let e = found.get(&n).cloned().unwrap_or(Entry {
                    v: "MATCH".into(),
                    lines: vec![],
                });
                (n, e)
            })
            .collect();
        let exact = entries.values().filter(|e| e.v == "MATCH").count();
        write_part(&o.tag, &part, &entries)?;
        println!("{part}: {} files, {exact} match", entries.len());
    }
    Ok(0)
}

#[derive(Default)]
struct Tally {
    exact: u64,
    gap: u64,
    fp: u64,
}
fn tally(e: &Entry) -> Tally {
    let mut t = Tally::default();
    for l in &e.lines {
        match l.result.as_str() {
            "exact" => t.exact += 1,
            "gap" => t.gap += 1,
            "fp" => t.fp += 1,
            _ => {}
        }
    }
    t
}

#[derive(PartialEq, Clone, Copy)]
enum Judgement {
    Same,
    Worse,
    Better,
    Changed,
}

fn judge(old: &Entry, new: &Entry) -> Judgement {
    if old == new {
        return Judgement::Same;
    }
    let (a, b) = (tally(old), tally(new));
    if b.exact < a.exact || b.fp > a.fp || (old.v == "MATCH" && new.v != "MATCH") {
        Judgement::Worse
    } else if b.exact > a.exact || b.fp < a.fp || b.gap < a.gap {
        Judgement::Better
    } else {
        Judgement::Changed
    }
}

fn verify(o: &Opts) -> Result<u8, String> {
    let reports = load_reports(&reports_dir(o))?;
    let empty = Entry {
        v: "MATCH".into(),
        lines: vec![],
    };
    let mut kinds: [Vec<(String, String, Entry, Entry)>; 4] = Default::default();
    for (part, found) in &reports {
        let recorded = read_part(&o.tag, part)?;
        let names: std::collections::BTreeSet<&String> =
            recorded.keys().chain(found.keys()).collect();
        for name in names {
            let old = recorded.get(name).unwrap_or(&empty);
            let new = found.get(name).unwrap_or(&empty);
            let slot = match judge(old, new) {
                Judgement::Same => 0,
                Judgement::Worse => 1,
                Judgement::Better => 2,
                Judgement::Changed => 3,
            };
            kinds[slot].push((part.clone(), name.clone(), old.clone(), new.clone()));
        }
    }
    let total: usize = kinds.iter().map(Vec::len).sum();
    println!(
        "{total} files compared with the map: {} same, {} better, {} changed, {} worse",
        kinds[0].len(),
        kinds[2].len(),
        kinds[3].len(),
        kinds[1].len()
    );
    let show = |slot: usize, label: &str, limit: usize, word: &str| {
        for (part, name, old, new) in kinds[slot].iter().take(limit) {
            let (a, b) = (tally(old), tally(new));
            println!(
                "  {label} {part}/{name}: exact {}->{}, gap {}->{}, false positive {}->{}, {} -> {}",
                a.exact, b.exact, a.gap, b.gap, a.fp, b.fp, old.v, new.v
            );
        }
        if kinds[slot].len() > limit {
            println!("  ... and {} more {word}", kinds[slot].len() - limit);
        }
    };
    show(1, "WORSE  ", 40, "worse");
    show(2, "better ", 15, "better");
    show(3, "changed", 15, "changed");
    if !kinds[1].is_empty() {
        println!(
            "\nSomething got worse than the map records. Fix it, or if it is intended, run `cargo xtask corpus update` and explain it in the pull request."
        );
        return Ok(1);
    }
    if !kinds[2].is_empty() || !kinds[3].is_empty() {
        println!(
            "\nNo regressions, but the map is out of date. Run `cargo xtask corpus run` then `cargo xtask corpus update` and commit tests/corpus/."
        );
        return Ok(3);
    }
    println!("the map and this run agree.");
    Ok(0)
}

// ---- status ------------------------------------------------------------------------------------

/// Thousands separators, as in 10,083.
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Counts keyed by diagnostic code, remembering first-seen order so ties list the same way every time.
#[derive(Default)]
struct Counts(Vec<(String, u64)>);
impl Counts {
    fn add(&mut self, code: &str) {
        match self.0.iter_mut().find(|(c, _)| c == code) {
            Some((_, n)) => *n += 1,
            None => self.0.push((code.to_string(), 1)),
        }
    }
    fn top(&self) -> String {
        let mut v: Vec<&(String, u64)> = self.0.iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v.iter()
            .take(5)
            .map(|(c, n)| format!("{c} ({n})"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn status_markdown(tag: &str) -> Result<String, String> {
    let (mut files, mut matched) = (0u64, 0u64);
    let (mut exact, mut gap, mut fp, mut other) = (0u64, 0u64, 0u64, 0u64);
    let (mut fp_codes, mut gap_codes) = (Counts::default(), Counts::default());
    let code_of = |d: &String| d.split(' ').next().unwrap_or("").to_string();
    for part in part_names(tag) {
        for entry in read_part(tag, &part)?.values() {
            files += 1;
            matched += (entry.v == "MATCH") as u64;
            for l in &entry.lines {
                match l.result.as_str() {
                    "exact" => exact += 1,
                    "gap" => {
                        gap += 1;
                        l.tsc.iter().for_each(|d| gap_codes.add(&code_of(d)));
                    }
                    "fp" => {
                        fp += 1;
                        l.tsox.iter().for_each(|d| fp_codes.add(&code_of(d)));
                    }
                    _ => other += 1,
                }
            }
        }
    }
    if files == 0 {
        return Err(format!("no map for {tag}"));
    }
    let total = exact + gap + fp + other;
    Ok(format!(
        "| | |\n| --- | --- |\n\
         | Files where tsox and tsc agree exactly | **{} of {}** ({:.1}%) |\n\
         | Lines with a diagnostic from either side | {}, of which {} agree |\n\
         | Gaps (tsc reports, tsox misses) | {} lines; most often {} |\n\
         | False positives (tsox reports, tsc does not) | {} lines; most often {} |\n\
         | Corpus | TypeScript {tag}, recorded in `tests/corpus/typescript-{}` |\n",
        group(matched),
        group(files),
        100.0 * matched as f64 / files as f64,
        group(total),
        group(exact),
        group(gap),
        gap_codes.top(),
        group(fp),
        fp_codes.top(),
        tag_dir(tag)
    ))
}

fn status(o: &Opts) -> Result<u8, String> {
    let text = status_markdown(&o.tag)?;
    println!("{text}");
    if o.readme {
        let path = crate::root().join("README.md");
        let readme = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let (start, end) = ("<!-- corpus-status:start -->", "<!-- corpus-status:end -->");
        let a = readme
            .find(start)
            .ok_or("README.md has no corpus-status block")?
            + start.len();
        let b = readme
            .find(end)
            .ok_or("README.md has no corpus-status block")?;
        fs::write(&path, format!("{}\n{}{}", &readme[..a], text, &readme[b..]))
            .map_err(|e| e.to_string())?;
    }
    Ok(0)
}

// ---- run ---------------------------------------------------------------------------------------

/// Runs tsox over the parts through compare.js, which reads tsc's answers from the map. Parts run in
/// parallel; compare.js is the one piece that stays Node, because it is what drives the real tsc.
fn run_parts(o: &Opts) -> Result<u8, String> {
    let data = corpus_dir(o);
    if !data.join(".complete").exists() {
        fetch(o)?;
    }
    let root = crate::root();
    let binary = std::env::var("TSOX_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target/release/tsox"));
    if !binary.exists() {
        return Err(format!(
            "{} does not exist: cargo build --release --bin tsox --features module-resolution",
            binary.display()
        ));
    }
    let tool = root.join("scripts/ts-diag-tool");
    if !tool.join("node_modules").is_dir() {
        let ok = Command::new("npm")
            .args(["ci", "--no-audit", "--no-fund"])
            .current_dir(&tool)
            .status()
            .map_err(|e| format!("npm: {e}"))?
            .success();
        if !ok {
            return Err("npm ci failed".into());
        }
    }
    let reports = o.reports.clone().unwrap_or_else(|| o.dest.join("reports"));
    let _ = fs::remove_dir_all(&reports);
    fs::create_dir_all(&reports).map_err(|e| e.to_string())?;
    let mut wanted: Vec<String> = fs::read_dir(&data)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|p| {
            p.starts_with("part-") && (o.parts.is_empty() || o.parts.iter().any(|w| w == &p[5..]))
        })
        .collect();
    wanted.sort();
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 4);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(part) = wanted.get(i) else { return };
                    if failure.lock().unwrap().is_some() {
                        return;
                    }
                    let out = fs::File::create(reports.join(format!("{part}.json")))
                        .expect("report file");
                    let result = Command::new("node")
                        .arg(tool.join("compare.js"))
                        .arg(data.join(part))
                        .arg(&binary)
                        .args(["--json", "--reference-map"])
                        .arg(map_dir(&o.tag))
                        .current_dir(&root)
                        .stdout(out)
                        .stderr(Stdio::piped())
                        .output();
                    match result {
                        Ok(r) if matches!(r.status.code(), Some(0) | Some(1)) => {
                            println!("ran {part}")
                        }
                        Ok(r) => {
                            let err = String::from_utf8_lossy(&r.stderr);
                            let tail: String = err
                                .chars()
                                .rev()
                                .take(2000)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect();
                            *failure.lock().unwrap() = Some(format!(
                                "{part}: compare.js failed ({:?}):\n{tail}",
                                r.status.code()
                            ));
                        }
                        Err(e) => *failure.lock().unwrap() = Some(format!("{part}: node: {e}")),
                    }
                }
            });
        }
    });
    match failure.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(0),
    }
}

pub fn run(args: &[String]) -> u8 {
    let o = parse(args.get(1..).unwrap_or(&[]));
    let result = match args.first().map(String::as_str) {
        Some("fetch") => fetch(&o).map(|_| 0),
        Some("run") => run_parts(&o),
        Some("verify") => verify(&o),
        Some("status") => status(&o),
        Some("build") | Some("update") => build(&o),
        _ => {
            println!(
                "{}",
                include_str!("corpus.rs")
                    .lines()
                    .skip(2)
                    .take_while(|l| l.starts_with("//"))
                    .map(|l| l.trim_start_matches("//").trim_start_matches(' '))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            Ok(2)
        }
    };
    result.unwrap_or_else(|e| {
        eprintln!("corpus: {e}");
        1
    })
}
