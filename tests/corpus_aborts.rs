#![cfg(feature = "module-resolution")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Problem: a stack overflow or an out-of-memory kill is an abort, not a panic, so the worker
// cannot catch it and one input can take the whole run down. A script that ran each corpus
// file alone could find them, but nothing ran it, so a new crash went unnoticed until
// someone did.
// Picked: run the real binary on one TypeScript 6.0.3 corpus file at a time, in its own
// process and its own project directory, and fail on a signal, a panic exit or a timeout,
// which are the three ways a run is lost. Any exit code the program chooses (0 clean,
// 1 diagnostics, 2 a file it could not check) is a normal result and is not judged here.
// Cost: a process per file. By default one file in 500 is run, which is a fixed sample of
// about twenty and takes seconds; TSOX_CORPUS_STRIDE=1 runs all 10,083, and
// TSOX_CORPUS_OFFSET shifts the sample so a different slice can be tried.
// Now: the eight cases that used to abort in project mode (known-crashers-6.0.3) are run
// one at a time in every run, so a regression there fails the build.

const TIMEOUT: Duration = Duration::from_secs(120);

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/typescript-testcases")
}

fn ts_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return files;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
        .collect();
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

enum Run {
    Finished(i32),
    Signal,
    TimedOut,
}

// Runs the binary on `file` copied into an empty project directory of its own.
fn run_alone(file: &Path, slot: &str) -> Run {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("corpus-aborts-{slot}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::copy(file, dir.join(file.file_name().unwrap())).unwrap();
    fs::write(dir.join("tsconfig.json"), "{}").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_tsox"))
        .arg("--project")
        .arg(dir.join("tsconfig.json"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary starts");
    let started = Instant::now();
    let result = loop {
        match child.try_wait().expect("the child can be polled") {
            Some(status) => {
                break match status.code() {
                    Some(code) => Run::Finished(code),
                    None => Run::Signal,
                };
            }
            None if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break Run::TimedOut;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let _ = fs::remove_dir_all(&dir);
    result
}

// A lost run: killed by a signal (a stack overflow aborts with SIGSEGV or SIGABRT), a panic
// that got out of the worker (exit 101), or no end in sight.
fn lost(run: &Run) -> Option<String> {
    match run {
        Run::Signal => Some("killed by a signal (abort or stack overflow)".to_string()),
        Run::TimedOut => Some(format!("no result after {} seconds", TIMEOUT.as_secs())),
        Run::Finished(code) if *code >= 100 => Some(format!("exit code {code}")),
        Run::Finished(_) => None,
    }
}

fn env_number(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[test]
fn a_sample_of_the_corpus_runs_to_an_end_one_file_at_a_time() {
    let cases = ts_files(&corpus().join("typescript-6.0.3"));
    // The corpus is part of the repository. A test that passes because it found nothing to
    // run says nothing, so a checkout without it fails here and does not look green.
    assert!(
        !cases.is_empty(),
        "tests/typescript-testcases/typescript-6.0.3 has no .ts files"
    );
    let stride = env_number("TSOX_CORPUS_STRIDE", 500).max(1);
    let offset = env_number("TSOX_CORPUS_OFFSET", 0);
    let mut failures = Vec::new();
    let mut ran = 0;
    for (index, file) in cases.iter().enumerate() {
        if index % stride != offset % stride {
            continue;
        }
        ran += 1;
        if let Some(why) = lost(&run_alone(file, "sample")) {
            failures.push(format!("{}: {why}", file.display()));
        }
    }
    assert!(ran > 0, "the sample was empty");
    assert!(
        failures.is_empty(),
        "{} of {ran} files lost the run:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_known_crashers_no_longer_abort_when_run_one_at_a_time() {
    let cases = ts_files(&corpus().join("known-crashers-6.0.3"));
    assert!(
        !cases.is_empty(),
        "tests/typescript-testcases/known-crashers-6.0.3 has no .ts files"
    );
    let mut failures = Vec::new();
    for file in &cases {
        if let Some(why) = lost(&run_alone(file, "crashers")) {
            failures.push(format!("{}: {why}", file.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
