//! Compares tsox with tsc and tsgo on one real project: speed, memory and diagnostics.
//!
//! One run builds a results folder under `.harness/results/<UTC time>/` holding everything
//! needed to check the numbers by hand: the raw output of every run, `summary.json`, a readable
//! `report.md`, the diagnostics tsox got wrong (`tsox-false-positives.txt`) or missed
//! (`tsox-missed.txt`), a per-file table (`per-file.csv`), and `REPRODUCE.sh` with the exact
//! commands.
//!
//! What it measures
//! - Speed: wall time of each compiler on the same project and tsconfig, after warm-up runs,
//!   with the runs of the three compilers interleaved (so a slow minute hurts all of them
//!   alike). Median, mean, standard deviation, min, max, quartiles and a noise flag.
//! - Memory and CPU: peak resident memory, user and system CPU time, from `wait4` on the child
//!   process, the same way for all three. No sampling, so a short spike is not missed.
//! - Agreement: tsox's errors against tsc's (the reference) and against tsgo's, matched by
//!   file, line and tsc error code. Counts of exact agreement, line agreement, false positives
//!   (tsox errors the reference does not have), gaps (reference errors tsox misses), and
//!   mixed files. tsc and tsgo are also compared with each other: if the two references
//!   disagree, the reference itself is uncertain and the report says so.
//! - Coverage: what tsox did not check. Every `TSR9xxx` warning is a "not checked yet" marker
//!   and every partial export is a file whose exports are incomplete, so both are listed.
//! - Scope: how many project files each compiler looked at. A smaller scope is not a speed win.
//!
//! What it does not claim: matching diagnostics are not proof of equal semantics, and tsox
//! checks a smaller language than tsc, so a speed ratio is only meaningful next to the
//! agreement numbers. The report states this next to the numbers.
//!
//! Usage: `./scripts/harness.sh [PROJECT_DIR] [options]` (see `--help`). Linux and macOS, 64-bit.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use xxhash_rust::xxh3::xxh3_128;

const ZUSTAND_URL: &str = "https://github.com/pmndrs/zustand.git";
const HELP: &str = "\
harness: compare tsox with tsc and tsgo on a real project (speed, memory, diagnostics)

USAGE:
    harness [PROJECT_DIR] [OPTIONS]

PROJECT_DIR is a TypeScript project with a tsconfig.json. Without it the harness clones and
uses Zustand under .harness/zustand.

OPTIONS:
    --runs N             timed runs per compiler (default 5)
    --warmup N           untimed runs per compiler before timing (default 1)
    --timeout SECS       kill a run that takes longer (default 600)
    --ref REF            Zustand branch, tag or commit when cloning (default main)
    --refresh            refetch the cached Zustand checkout at --ref
    --tsconfig FILE      tsconfig to use, relative to the project (default tsconfig.json)
    --tsox PATH          tsox binary (default <target dir>/release/tsox)
    --tsc CMD            classic tsc command (default: typescript installed under .harness/tools)
    --tsgo CMD           tsgo command (default: native binary from typescript@7 under .harness/tools)
    --tsc-version V      typescript version to install for tsc (default 6.0.3)
    --tsgo-version V     typescript version to install for tsgo (default 7)
    --no-install         do not install project dependencies or compilers
    --tsox-only          measure tsox alone: no tsc, no tsgo, no comparison (time, memory,
                         diagnostics by code, determinism)
    --dhat               also run examples/dhat_heap (allocation profile of the benchmark workloads)
    --out DIR            results folder (default .harness/results/<UTC time>)
    --cleanup            delete the cloned Zustand checkout afterwards
    --strict             exit 1 when the run is invalid or tsox has false positives
    -h, --help           show this message

Environment fallbacks: HARNESS_RUNS, HARNESS_WARMUP, HARNESS_TIMEOUT, HARNESS_CARGO_FLAGS (extra\ncargo flags for --dhat), ZUSTAND_REF,
HARNESS_REFRESH, SKIP_INSTALL, CLEANUP, TSC_BIN, TSGO_BIN, TSOX_BIN, HARNESS_TSC_VERSION,
HARNESS_TSGO_VERSION.

EXIT CODES: 0 done, 1 --strict failed, 2 setup error.
";

// ---------------------------------------------------------------------------------------------
// Running a child process and reading what it cost.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Exit {
    code: Option<i32>,
    signal: Option<i32>,
}

impl Exit {
    fn describe(&self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("exit {code}"),
            (None, Some(signal)) => format!("signal {signal}"),
            _ => "unknown".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Usage {
    user_ms: f64,
    sys_ms: f64,
    max_rss_mb: f64,
}

struct RawRun {
    exit: Exit,
    usage: Option<Usage>,
    wall_ms: f64,
    /// Peak memory of the measuring process itself when it started the target: memory below
    /// this cannot be told apart from it.
    floor_mb: f64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

/// What the measuring process writes about one run, as `key=value` lines.
#[derive(Debug, Default)]
struct Measured {
    wall_ms: f64,
    user_ms: f64,
    sys_ms: f64,
    max_rss_mb: f64,
    floor_mb: f64,
    exit_code: Option<i32>,
    signal: Option<i32>,
    timed_out: bool,
}

fn parse_measured(text: &str) -> Result<Measured, String> {
    let mut measured = Measured::default();
    let mut seen_wall = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let number = |value: &str| -> Result<f64, String> {
            value
                .parse()
                .map_err(|_| format!("bad number for {key}: {value}"))
        };
        match key {
            "error" => return Err(value.to_owned()),
            "wall_ms" => {
                measured.wall_ms = number(value)?;
                seen_wall = true;
            }
            "user_ms" => measured.user_ms = number(value)?,
            "sys_ms" => measured.sys_ms = number(value)?,
            "max_rss_mb" => measured.max_rss_mb = number(value)?,
            "floor_mb" => measured.floor_mb = number(value)?,
            "exit_code" => measured.exit_code = Some(number(value)? as i32),
            "signal" => measured.signal = Some(number(value)? as i32),
            "timed_out" => measured.timed_out = value == "1",
            _ => {}
        }
    }
    if seen_wall {
        Ok(measured)
    } else {
        Err("the measuring process wrote no timing".to_owned())
    }
}

#[cfg(all(unix, target_pointer_width = "64"))]
mod sys {
    //! Why a launcher process: the kernel counts a child's peak memory from the moment it is
    //! created, and a child created by a big process starts with that process's peak. Measured
    //! straight from the harness, a 27 MB program read 29 MB, and from a parent holding 150 MB
    //! it read 161 MB. So the harness runs itself in a tiny `--__measure` mode that spawns the
    //! target and calls `wait4`; its own size at that moment is reported as the floor.
    use super::{RawRun, Usage, parse_measured};
    use std::fs;
    use std::io::{self, Read};
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Timeval {
        sec: i64,
        // `suseconds_t` is 8 bytes on Linux and 4 (plus padding) on macOS; the value is under a
        // million, so its low 32 bits are right on both little-endian layouts.
        usec: i64,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Rusage {
        utime: Timeval,
        stime: Timeval,
        maxrss: i64,
        rest: [i64; 13],
    }

    unsafe extern "C" {
        fn wait4(pid: i32, status: *mut i32, options: i32, rusage: *mut Rusage) -> i32;
        fn getrusage(who: i32, usage: *mut Rusage) -> i32;
        fn kill(pid: i32, sig: i32) -> i32;
    }

    fn wait_for(pid: i32) -> io::Result<(i32, Rusage)> {
        loop {
            let mut status = 0i32;
            let mut usage = Rusage::default();
            // SAFETY: both pointers are to live locals of the right C layout, and `pid` is a
            // child this process spawned and has not waited for.
            let waited = unsafe { wait4(pid, &mut status, 0, &mut usage) };
            if waited == pid {
                return Ok((status, usage));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn millis(t: Timeval) -> f64 {
        t.sec as f64 * 1000.0 + (t.usec as i32) as f64 / 1000.0
    }

    /// Linux reports kilobytes, macOS bytes.
    fn rss_mb(maxrss: i64) -> f64 {
        if cfg!(target_os = "macos") {
            maxrss as f64 / (1024.0 * 1024.0)
        } else {
            maxrss as f64 / 1024.0
        }
    }

    /// The peak memory of this process's own address space. `getrusage` also counts the
    /// memory of the process that started us (it is carried over by `exec`), so on Linux the
    /// kernel's own high-water mark for the current address space is read instead.
    fn own_peak_mb() -> f64 {
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            let kb = status
                .lines()
                .find_map(|line| line.strip_prefix("VmHWM:"))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|n| n.parse::<f64>().ok());
            if let Some(kb) = kb {
                return kb / 1024.0;
            }
        }
        let mut usage = Rusage::default();
        // SAFETY: a live local of the right C layout; 0 is RUSAGE_SELF.
        let ok = unsafe { getrusage(0, &mut usage) };
        if ok == 0 { rss_mb(usage.maxrss) } else { 0.0 }
    }

    /// `--__measure OUT TIMEOUT_MS -- PROGRAM ARGS...`: run the program with the output going
    /// where this process's output goes, and write what it cost to the file OUT.
    pub fn launcher_main(args: &[String]) -> i32 {
        if args.len() < 4 || args[2] != "--" {
            eprintln!("usage: --__measure OUT TIMEOUT_MS -- PROGRAM [ARGS...]");
            return 2;
        }
        let out = &args[0];
        let timeout = Duration::from_millis(args[1].parse().unwrap_or(600_000));
        let program = &args[3];
        let floor = own_peak_mb();

        let mut command = Command::new(program);
        command
            .args(&args[4..])
            .stdin(Stdio::null())
            .process_group(0);
        let started = Instant::now();
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::write(out, format!("error=cannot start `{program}`: {error}\n"));
                return 0;
            }
        };
        let pid = child.id() as i32;
        let done = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let watchdog = {
            let (done, timed_out) = (Arc::clone(&done), Arc::clone(&timed_out));
            thread::spawn(move || {
                let begun = Instant::now();
                while !done.load(Ordering::Relaxed) {
                    if begun.elapsed() >= timeout {
                        timed_out.store(true, Ordering::Relaxed);
                        // SAFETY: a plain signal to the process group this run created.
                        unsafe { kill(-pid, 9) };
                        return;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            })
        };
        let waited = wait_for(pid);
        let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        done.store(true, Ordering::Relaxed);
        let _ = watchdog.join();
        // The child is already reaped; dropping the handle must not wait again.
        std::mem::forget(child);

        let mut text = String::new();
        match waited {
            Ok((status, usage)) => {
                text.push_str(&format!("wall_ms={wall_ms:.3}\n"));
                text.push_str(&format!("user_ms={:.3}\n", millis(usage.utime)));
                text.push_str(&format!("sys_ms={:.3}\n", millis(usage.stime)));
                text.push_str(&format!("max_rss_mb={:.3}\n", rss_mb(usage.maxrss)));
                text.push_str(&format!("floor_mb={floor:.3}\n"));
                if status & 0x7f == 0 {
                    text.push_str(&format!("exit_code={}\n", (status >> 8) & 0xff));
                } else {
                    text.push_str(&format!("signal={}\n", status & 0x7f));
                }
                text.push_str(&format!(
                    "timed_out={}\n",
                    u8::from(timed_out.load(Ordering::Relaxed))
                ));
            }
            Err(error) => text.push_str(&format!("error=wait failed: {error}\n")),
        }
        if fs::write(out, text).is_err() {
            return 2;
        }
        0
    }

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    pub fn run_measured(argv: &[String], cwd: &Path, timeout: Duration) -> Result<RawRun, String> {
        if argv.is_empty() {
            return Err("empty command".to_owned());
        }
        let exe = std::env::current_exe().map_err(|e| format!("cannot find the harness: {e}"))?;
        let out_path = std::env::temp_dir().join(format!(
            "harness-measure-{}-{}.txt",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut command = Command::new(exe);
        command
            .arg("--__measure")
            .arg(&out_path)
            .arg(timeout.as_millis().to_string())
            .arg("--")
            .args(argv)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start the measuring process: {error}"))?;
        let mut out_pipe = child.stdout.take().ok_or("no stdout pipe")?;
        let mut err_pipe = child.stderr.take().ok_or("no stderr pipe")?;
        let out_reader = thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = out_pipe.read_to_end(&mut buffer);
            buffer
        });
        let err_reader = thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = err_pipe.read_to_end(&mut buffer);
            buffer
        });
        let launcher = child.wait().map_err(|e| format!("wait failed: {e}"))?;
        let stdout = out_reader.join().unwrap_or_default();
        let stderr = err_reader.join().unwrap_or_default();
        let text = fs::read_to_string(&out_path);
        let _ = fs::remove_file(&out_path);
        let text =
            text.map_err(|e| format!("the measuring process left no result ({launcher}): {e}"))?;
        let measured = parse_measured(&text)?;
        Ok(RawRun {
            exit: super::Exit {
                code: measured.exit_code,
                signal: measured.signal,
            },
            usage: Some(Usage {
                user_ms: measured.user_ms,
                sys_ms: measured.sys_ms,
                max_rss_mb: measured.max_rss_mb,
            }),
            wall_ms: measured.wall_ms,
            floor_mb: measured.floor_mb,
            stdout,
            stderr,
            timed_out: measured.timed_out,
        })
    }
}

#[cfg(not(all(unix, target_pointer_width = "64")))]
mod sys {
    use super::RawRun;
    use std::path::Path;
    use std::time::Duration;

    pub fn launcher_main(_: &[String]) -> i32 {
        2
    }

    pub fn run_measured(_: &[String], _: &Path, _: Duration) -> Result<RawRun, String> {
        Err("the harness measures with wait4 and supports 64-bit Linux and macOS only".to_owned())
    }
}

fn run_measured(argv: &[String], cwd: &Path, timeout: Duration) -> Result<RawRun, String> {
    sys::run_measured(argv, cwd, timeout)
}

/// Runs a setup command with its output shown, failing on a nonzero exit.
fn run_visible(argv: &[&str], cwd: &Path) -> Result<(), String> {
    let status = Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .status()
        .map_err(|error| format!("cannot start `{}`: {error}", argv[0]))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{}` failed ({status})", argv.join(" ")))
    }
}

/// Runs a command and returns its trimmed stdout, or None when it fails.
fn capture(argv: &[&str], cwd: &Path) -> Option<String> {
    let output = Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

// ---------------------------------------------------------------------------------------------
// Diagnostics: parsing what each compiler prints.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Diag {
    file: String,
    line: u32,
    col: u32,
    code: String,
    message: String,
}

fn rel_path(root: &Path, raw: &str) -> String {
    let path = Path::new(raw);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).unwrap_or(path).to_path_buf()
    } else {
        path.to_path_buf()
    };
    let text = relative.to_string_lossy().replace('\\', "/");
    text.strip_prefix("./").unwrap_or(&text).to_owned()
}

/// A file inside the project, as opposed to a dependency, a library or "no file at all".
fn is_project_file(relative: &str) -> bool {
    !relative.is_empty()
        && !relative.starts_with('/')
        && !relative.starts_with("../")
        && !relative.contains("node_modules/")
}

/// `path(line,col): error TS1234: message`, the shape tsc and tsgo print with `--pretty false`.
/// An error with no location (`error TS2688: ...`) has an empty file and line 0.
fn parse_ts_line(line: &str) -> Option<(String, u32, u32, String, String)> {
    if let Some(rest) = line.strip_prefix("error TS") {
        let (digits, message) = rest.split_once(": ")?;
        return Some((
            String::new(),
            0,
            0,
            format!("TS{digits}"),
            message.to_owned(),
        ));
    }
    let at = line.find("): error TS")?;
    let (head, rest) = line.split_at(at);
    let open = head.rfind('(')?;
    let path = &head[..open];
    let mut position = head[open + 1..].split(',');
    let line_no: u32 = position.next()?.trim().parse().ok()?;
    let col: u32 = position.next()?.trim().parse().ok()?;
    let rest = &rest["): error ".len()..];
    let (code, message) = rest.split_once(": ")?;
    Some((
        path.to_owned(),
        line_no,
        col,
        code.to_owned(),
        message.to_owned(),
    ))
}

fn parse_ts_output(text: &str, root: &Path) -> Vec<Diag> {
    let mut diags: Vec<Diag> = Vec::new();
    for raw in text.lines() {
        if let Some((file, line, col, code, message)) = parse_ts_line(raw) {
            diags.push(Diag {
                file: rel_path(root, &file),
                line,
                col,
                code,
                message,
            });
        } else if raw.starts_with(char::is_whitespace) && !raw.trim().is_empty() {
            if let Some(last) = diags.last_mut() {
                last.message.push('\n');
                last.message.push_str(raw);
            }
        }
    }
    diags
}

#[derive(Default)]
struct TsoxParse {
    errors: Vec<Diag>,
    warnings: Vec<Diag>,
    checked: Option<usize>,
    graph_files: Option<usize>,
    unresolved_imports: Option<usize>,
    cycles: Option<usize>,
    reported_errors: Option<usize>,
    reported_warnings: Option<usize>,
    partial: Vec<(String, String)>,
}

/// `path:line:col: error: TS2322 message` (with `--tsc-codes`), plus the summary lines.
fn parse_tsox_line(line: &str) -> Option<(bool, String, u32, u32, String, String)> {
    let (at, is_error, marker) = if let Some(at) = line.find(": error: ") {
        (at, true, ": error: ")
    } else if let Some(at) = line.find(": warning: ") {
        (at, false, ": warning: ")
    } else {
        return None;
    };
    let tail = &line[at + marker.len()..];
    let (code, message) = tail.split_once(' ')?;
    if !code.starts_with("TS") {
        return None;
    }
    let mut parts = line[..at].rsplitn(3, ':');
    let col: u32 = parts.next()?.parse().ok()?;
    let line_no: u32 = parts.next()?.parse().ok()?;
    let path = parts.next()?;
    Some((
        is_error,
        path.to_owned(),
        line_no,
        col,
        code.to_owned(),
        message.to_owned(),
    ))
}

fn number_before(text: &str, suffix: &str) -> Option<usize> {
    let end = text.find(suffix)?;
    let digits: String = text[..end]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

fn parse_tsox_output(text: &str, root: &Path) -> TsoxParse {
    let mut parsed = TsoxParse::default();
    let mut last_is_error: Option<bool> = None;
    for raw in text.lines() {
        if let Some((is_error, file, line, col, code, message)) = parse_tsox_line(raw) {
            let diag = Diag {
                file: rel_path(root, &file),
                line,
                col,
                code,
                message,
            };
            if is_error {
                parsed.errors.push(diag);
            } else {
                parsed.warnings.push(diag);
            }
            last_is_error = Some(is_error);
        } else if raw.starts_with(char::is_whitespace) && !raw.trim().is_empty() {
            let target = match last_is_error {
                Some(true) => parsed.errors.last_mut(),
                Some(false) => parsed.warnings.last_mut(),
                None => None,
            };
            if let Some(last) = target {
                last.message.push('\n');
                last.message.push_str(raw);
            }
        } else if let Some(rest) = raw.strip_prefix("Checked ") {
            last_is_error = None;
            parsed.checked = number_before(rest, " file(s)");
            parsed.reported_errors = number_before(rest, " error(s)");
            parsed.reported_warnings = number_before(rest, " warning(s)");
        } else if let Some(rest) = raw.strip_prefix("Module graph: ") {
            last_is_error = None;
            parsed.graph_files = number_before(rest, " file(s)");
            parsed.cycles = number_before(rest, " cycle(s)");
            parsed.unresolved_imports = number_before(rest, " unresolved import(s)");
        } else if let Some((file, reason)) = raw.split_once(": exports partial: ") {
            last_is_error = None;
            parsed
                .partial
                .push((rel_path(root, file), reason.to_owned()));
        } else {
            last_is_error = None;
        }
    }
    parsed
}

/// Lines such as `Files:   312` or `Check time:  2.72s` from `--extendedDiagnostics`.
fn parse_extended(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in text.lines() {
        if let Some((key, value)) = raw.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            let known = !key.is_empty()
                && key.len() < 40
                && !value.is_empty()
                && value.chars().next().is_some_and(|c| c.is_ascii_digit())
                && !key.contains('(')
                && !key.contains('/');
            if known {
                map.insert(key.to_owned(), value.to_owned());
            }
        }
    }
    map
}

// ---------------------------------------------------------------------------------------------
// Comparing two sets of diagnostics.
// ---------------------------------------------------------------------------------------------

type Key = (String, u32, String);

fn tally<'a>(diags: impl IntoIterator<Item = &'a Diag>) -> BTreeMap<Key, usize> {
    let mut counts = BTreeMap::new();
    for diag in diags {
        *counts
            .entry((diag.file.clone(), diag.line, diag.code.clone()))
            .or_insert(0) += 1;
    }
    counts
}

#[derive(Default)]
struct Comparison {
    reference_total: usize,
    candidate_total: usize,
    matched: usize,
    /// Reported by the candidate, not by the reference.
    false_positives: Vec<(Key, usize)>,
    /// Reported by the reference, not by the candidate.
    missed: Vec<(Key, usize)>,
    lines_both: usize,
    lines_reference_only: usize,
    lines_candidate_only: usize,
    /// Lines both report, with different codes or counts.
    lines_code_differs: usize,
}

impl Comparison {
    fn false_positive_total(&self) -> usize {
        self.false_positives.iter().map(|(_, n)| n).sum()
    }
    fn missed_total(&self) -> usize {
        self.missed.iter().map(|(_, n)| n).sum()
    }
    /// Share of the candidate's errors the reference also has. None with no candidate errors.
    fn precision(&self) -> Option<f64> {
        (self.candidate_total > 0).then(|| self.matched as f64 / self.candidate_total as f64)
    }
    /// Share of the reference's errors the candidate also has. None with no reference errors.
    fn recall(&self) -> Option<f64> {
        (self.reference_total > 0).then(|| self.matched as f64 / self.reference_total as f64)
    }
}

fn compare(reference: &[Diag], candidate: &[Diag]) -> Comparison {
    let want = tally(reference);
    let got = tally(candidate);
    let mut result = Comparison {
        reference_total: reference.len(),
        candidate_total: candidate.len(),
        ..Comparison::default()
    };
    for (key, &count) in &got {
        let other = want.get(key).copied().unwrap_or(0);
        result.matched += count.min(other);
        if count > other {
            result.false_positives.push((key.clone(), count - other));
        }
    }
    for (key, &count) in &want {
        let other = got.get(key).copied().unwrap_or(0);
        if count > other {
            result.missed.push((key.clone(), count - other));
        }
    }
    let lines = |diags: &[Diag]| -> BTreeMap<(String, u32), Vec<String>> {
        let mut map: BTreeMap<(String, u32), Vec<String>> = BTreeMap::new();
        for diag in diags {
            map.entry((diag.file.clone(), diag.line))
                .or_default()
                .push(diag.code.clone());
        }
        for codes in map.values_mut() {
            codes.sort();
        }
        map
    };
    let want_lines = lines(reference);
    let got_lines = lines(candidate);
    for (line, codes) in &got_lines {
        match want_lines.get(line) {
            Some(other) => {
                result.lines_both += 1;
                if other != codes {
                    result.lines_code_differs += 1;
                }
            }
            None => result.lines_candidate_only += 1,
        }
    }
    result.lines_reference_only = want_lines
        .keys()
        .filter(|line| !got_lines.contains_key(*line))
        .count();
    result
}

fn in_file<'a>(diags: &'a [Diag], file: &str) -> Vec<&'a Diag> {
    diags.iter().filter(|d| d.file == file).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FileClass {
    /// Same errors on the same lines with the same codes (including none on either side).
    Agree,
    /// Same lines, but different codes or counts.
    AgreeLines,
    /// The reference reports lines the candidate does not.
    Gap,
    /// The candidate reports lines the reference does not.
    FalsePositive,
    /// Both of the above.
    Mixed,
}

impl FileClass {
    fn label(self) -> &'static str {
        match self {
            FileClass::Agree => "agree",
            FileClass::AgreeLines => "agree-lines",
            FileClass::Gap => "gap",
            FileClass::FalsePositive => "false-positive",
            FileClass::Mixed => "mixed",
        }
    }
}

fn classify(reference: &[&Diag], candidate: &[&Diag]) -> FileClass {
    let want = tally(reference.iter().copied());
    let got = tally(candidate.iter().copied());
    if want == got {
        return FileClass::Agree;
    }
    let want_lines: BTreeSet<u32> = reference.iter().map(|d| d.line).collect();
    let got_lines: BTreeSet<u32> = candidate.iter().map(|d| d.line).collect();
    let only_reference = want_lines.difference(&got_lines).count();
    let only_candidate = got_lines.difference(&want_lines).count();
    match (only_reference > 0, only_candidate > 0) {
        (false, false) => FileClass::AgreeLines,
        (true, false) => FileClass::Gap,
        (false, true) => FileClass::FalsePositive,
        (true, true) => FileClass::Mixed,
    }
}

// ---------------------------------------------------------------------------------------------
// Statistics.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Stats {
    n: usize,
    min: f64,
    max: f64,
    mean: f64,
    median: f64,
    stdev: f64,
    q1: f64,
    q3: f64,
    /// Standard deviation over the mean, in percent.
    cv: f64,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = (sorted.len() - 1) as f64 * p;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (position - low as f64)
}

fn stats(values: &[f64]) -> Option<Stats> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let n = sorted.len();
    let mean = sorted.iter().sum::<f64>() / n as f64;
    let stdev = if n > 1 {
        (sorted.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
    } else {
        0.0
    };
    Some(Stats {
        n,
        min: sorted[0],
        max: sorted[n - 1],
        mean,
        median: percentile(&sorted, 0.5),
        stdev,
        q1: percentile(&sorted, 0.25),
        q3: percentile(&sorted, 0.75),
        cv: if mean > 0.0 {
            stdev / mean * 100.0
        } else {
            0.0
        },
    })
}

// ---------------------------------------------------------------------------------------------
// A small JSON writer, so the crate needs no serialization dependency.
// ---------------------------------------------------------------------------------------------

enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

fn obj(fields: Vec<(&str, J)>) -> J {
    J::Obj(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn jstr(text: impl Into<String>) -> J {
    J::Str(text.into())
}

fn jnum(value: f64) -> J {
    J::Num(value)
}

fn jopt(value: Option<f64>) -> J {
    value.map_or(J::Null, J::Num)
}

fn escape_json(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl J {
    fn render(&self, depth: usize, out: &mut String) {
        let pad = |n: usize| "  ".repeat(n);
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) if !n.is_finite() => out.push_str("null"),
            J::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => {
                let _ = write!(out, "{}", *n as i64);
            }
            J::Num(n) => {
                let _ = write!(out, "{n:.3}");
            }
            J::Str(s) => escape_json(s, out),
            J::Arr(items) if items.is_empty() => out.push_str("[]"),
            J::Arr(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    item.render(depth + 1, out);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(depth));
                out.push(']');
            }
            J::Obj(fields) if fields.is_empty() => out.push_str("{}"),
            J::Obj(fields) => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    escape_json(key, out);
                    out.push_str(": ");
                    value.render(depth + 1, out);
                    out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(depth));
                out.push('}');
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Small helpers: time, hashes, tables.
// ---------------------------------------------------------------------------------------------

/// Calendar fields of a Unix time: (year, month, day, hour, minute, second), UTC.
fn civil(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rest = (secs % 86_400) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rest / 3600, rest % 3600 / 60, rest % 60)
}

fn utc_stamp(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

fn utc_pretty(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = civil(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn fingerprint(bytes: &[u8]) -> String {
    format!("{:032x}", xxh3_128(bytes))
}

fn file_fingerprint(path: &Path) -> String {
    fs::read(path).map_or_else(|_| "unreadable".to_owned(), |b| fingerprint(&b))
}

fn md_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "| {} |", headers.join(" | "));
    let _ = writeln!(out, "|{}|", vec!["---"; headers.len()].join("|"));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|c| c.replace('|', "\\|")).collect();
        let _ = writeln!(out, "| {} |", cells.join(" | "));
    }
    out
}

fn f1(value: f64) -> String {
    format!("{value:.1}")
}

fn pct(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |v| format!("{:.1}%", v * 100.0))
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

fn one_line(text: &str) -> String {
    let line = first_line(text);
    if line.chars().count() > 140 {
        let cut: String = line.chars().take(137).collect();
        format!("{cut}...")
    } else {
        line
    }
}

// ---------------------------------------------------------------------------------------------
// Configuration.
// ---------------------------------------------------------------------------------------------

struct Args {
    project: Option<PathBuf>,
    runs: usize,
    warmup: usize,
    timeout: Duration,
    reference: String,
    refresh: bool,
    tsconfig: String,
    tsox: Option<String>,
    tsc: Option<String>,
    tsgo: Option<String>,
    tsc_version: String,
    tsgo_version: String,
    no_install: bool,
    tsox_only: bool,
    dhat: bool,
    out: Option<PathBuf>,
    cleanup: bool,
    strict: bool,
}

fn env_flag(name: &str) -> bool {
    env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn env_text(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn parse_args(argv: &[String]) -> Result<Option<Args>, String> {
    let mut args = Args {
        project: None,
        runs: env_text("HARNESS_RUNS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(5),
        warmup: env_text("HARNESS_WARMUP")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1),
        timeout: Duration::from_secs(
            env_text("HARNESS_TIMEOUT")
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
        ),
        reference: env_text("ZUSTAND_REF").unwrap_or_else(|| "main".to_owned()),
        refresh: env_flag("HARNESS_REFRESH"),
        tsconfig: "tsconfig.json".to_owned(),
        tsox: env_text("TSOX_BIN"),
        tsc: env_text("TSC_BIN"),
        tsgo: env_text("TSGO_BIN"),
        tsc_version: env_text("HARNESS_TSC_VERSION").unwrap_or_else(|| "6.0.3".to_owned()),
        tsgo_version: env_text("HARNESS_TSGO_VERSION").unwrap_or_else(|| "7".to_owned()),
        no_install: env_flag("SKIP_INSTALL"),
        tsox_only: env_flag("HARNESS_TSOX_ONLY"),
        dhat: false,
        out: None,
        cleanup: env_flag("CLEANUP"),
        strict: false,
    };
    let mut iter = argv.iter().peekable();
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = |label: &str| -> Result<String, String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => iter
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("{label} needs a value")),
            }
        };
        match name.as_str() {
            "-h" | "--help" => return Ok(None),
            "--runs" => {
                args.runs = value("--runs")?
                    .parse()
                    .map_err(|_| "--runs: not a number")?
            }
            "--warmup" => {
                args.warmup = value("--warmup")?
                    .parse()
                    .map_err(|_| "--warmup: not a number")?
            }
            "--timeout" => {
                args.timeout = Duration::from_secs(
                    value("--timeout")?
                        .parse()
                        .map_err(|_| "--timeout: not a number")?,
                )
            }
            "--ref" => args.reference = value("--ref")?,
            "--refresh" => args.refresh = true,
            "--tsconfig" => args.tsconfig = value("--tsconfig")?,
            "--tsox" => args.tsox = Some(value("--tsox")?),
            "--tsc" => args.tsc = Some(value("--tsc")?),
            "--tsgo" => args.tsgo = Some(value("--tsgo")?),
            "--tsc-version" => args.tsc_version = value("--tsc-version")?,
            "--tsgo-version" => args.tsgo_version = value("--tsgo-version")?,
            "--no-install" => args.no_install = true,
            "--tsox-only" => args.tsox_only = true,
            "--dhat" => args.dhat = true,
            "--out" => args.out = Some(PathBuf::from(value("--out")?)),
            "--cleanup" => args.cleanup = true,
            "--strict" => args.strict = true,
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            _ => {
                if args.project.is_some() {
                    return Err("only one PROJECT_DIR is accepted".to_owned());
                }
                args.project = Some(PathBuf::from(arg));
            }
        }
    }
    if args.runs == 0 {
        return Err("--runs must be at least 1".to_owned());
    }
    Ok(Some(args))
}

// ---------------------------------------------------------------------------------------------
// Setup: project, dependencies and the three compilers.
// ---------------------------------------------------------------------------------------------

struct Fixture {
    dir: PathBuf,
    owned: bool,
    description: String,
    revision: String,
    tsconfig: PathBuf,
}

fn looks_like_sha(text: &str) -> bool {
    (7..=40).contains(&text.len()) && text.chars().all(|c| c.is_ascii_hexdigit())
}

fn prepare_fixture(args: &Args, root: &Path) -> Result<Fixture, String> {
    let (dir, owned) = match &args.project {
        Some(path) => (
            path.canonicalize()
                .map_err(|e| format!("cannot open {}: {e}", path.display()))?,
            false,
        ),
        None => (root.join(".harness").join("zustand"), true),
    };
    if owned {
        fs::create_dir_all(dir.parent().unwrap_or(root)).map_err(|e| e.to_string())?;
        let reference = args.reference.as_str();
        if !dir.join(".git").exists() {
            eprintln!("Fetching Zustand {reference} (first run only)...");
            let dir_text = dir.to_string_lossy().into_owned();
            let cloned = if looks_like_sha(reference) {
                Err(String::new())
            } else {
                run_visible(
                    &[
                        "git",
                        "clone",
                        "--depth",
                        "1",
                        "--branch",
                        reference,
                        ZUSTAND_URL,
                        &dir_text,
                    ],
                    root,
                )
            };
            if cloned.is_err() {
                // A commit hash cannot be cloned with --branch; fetch it by hash instead.
                let _ = fs::remove_dir_all(&dir);
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                run_visible(&["git", "init", "-q"], &dir)?;
                run_visible(&["git", "remote", "add", "origin", ZUSTAND_URL], &dir)?;
                run_visible(&["git", "fetch", "--depth", "1", "origin", reference], &dir)
                    .map_err(|e| format!("could not fetch Zustand {reference}: {e}"))?;
                run_visible(&["git", "checkout", "-q", "--detach", "FETCH_HEAD"], &dir)?;
            }
        } else if args.refresh {
            eprintln!("Refreshing the cached Zustand checkout to {reference}...");
            run_visible(&["git", "fetch", "--depth", "1", "origin", reference], &dir)?;
            run_visible(&["git", "checkout", "-q", "--detach", "FETCH_HEAD"], &dir)?;
        }
    }
    let tsconfig = dir.join(&args.tsconfig);
    if !tsconfig.is_file() {
        return Err(format!("no {} in {}", args.tsconfig, dir.display()));
    }
    let revision = capture(&["git", "rev-parse", "HEAD"], &dir)
        .map(|sha| {
            let dirty =
                capture(&["git", "status", "--porcelain"], &dir).is_some_and(|s| !s.is_empty());
            if dirty {
                format!("{sha} (modified)")
            } else {
                sha
            }
        })
        .unwrap_or_else(|| "not a git checkout".to_owned());
    let description = if owned {
        format!("pmndrs/zustand @ {}", args.reference)
    } else {
        dir.display().to_string()
    };
    Ok(Fixture {
        dir,
        owned,
        description,
        revision,
        tsconfig,
    })
}

fn install_dependencies(fixture: &Fixture, args: &Args, root: &Path) -> Result<String, String> {
    let dir = &fixture.dir;
    if args.no_install {
        return Ok("skipped (--no-install)".to_owned());
    }
    let choices: [(&str, &str, bool); 3] = [
        ("pnpm", "pnpm-lock.yaml", have("pnpm")),
        ("npm", "package-lock.json", have("npm")),
        ("yarn", "yarn.lock", have("yarn")),
    ];
    let Some((manager, lockfile, _)) = choices
        .iter()
        .find(|(_, lock, available)| *available && dir.join(lock).is_file())
    else {
        return Ok("no supported lockfile; nothing installed".to_owned());
    };
    let marker_dir = root.join(".harness").join("install-markers");
    fs::create_dir_all(&marker_dir).map_err(|e| e.to_string())?;
    let marker = marker_dir.join(fingerprint(dir.to_string_lossy().as_bytes()));
    let current = format!("{manager}:{}", file_fingerprint(&dir.join(lockfile)));
    if dir.join("node_modules").is_dir()
        && fs::read_to_string(&marker).is_ok_and(|saved| saved.trim() == current)
    {
        return Ok(format!("{manager}, frozen lockfile (already installed)"));
    }
    eprintln!("Installing dependencies with {manager} (only when the lockfile changes)...");
    let frozen: Result<(), String> = match *manager {
        "pnpm" => run_visible(
            &["pnpm", "install", "--frozen-lockfile", "--ignore-scripts"],
            dir,
        ),
        "npm" => run_visible(
            &["npm", "ci", "--ignore-scripts", "--no-audit", "--no-fund"],
            dir,
        ),
        _ => run_visible(
            &["yarn", "install", "--frozen-lockfile", "--ignore-scripts"],
            dir,
        ),
    };
    let mode = match frozen {
        Ok(()) => format!("{manager}, frozen lockfile"),
        Err(first) if *manager == "npm" => {
            eprintln!(
                "{first}\nThe lockfile is out of sync with package.json; installing without it."
            );
            run_visible(
                &[
                    "npm",
                    "install",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                    "--no-package-lock",
                ],
                dir,
            )?;
            "npm, UNLOCKED (lockfile out of sync): dependency versions are not pinned".to_owned()
        }
        Err(error) => return Err(format!("dependency install failed: {error}")),
    };
    fs::write(&marker, format!("{current}\n")).map_err(|e| e.to_string())?;
    Ok(mode)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Tsc,
    Tsgo,
    Tsox,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Tsc => "tsc",
            Kind::Tsgo => "tsgo",
            Kind::Tsox => "tsox",
        }
    }
}

struct Tool {
    kind: Kind,
    argv: Vec<String>,
    version: String,
    notes: Vec<String>,
}

impl Tool {
    /// The command line of a timed run: errors only, plain text, no emit, the project's tsconfig.
    fn timed(&self, tsconfig: &str) -> Vec<String> {
        let mut argv = self.argv.clone();
        match self.kind {
            Kind::Tsc | Kind::Tsgo => {
                argv.extend(
                    ["--noEmit", "--pretty", "false", "-p", tsconfig]
                        .iter()
                        .map(|s| (*s).to_owned()),
                );
            }
            Kind::Tsox => {
                argv.extend(
                    ["--project", tsconfig, "--tsc-codes"]
                        .iter()
                        .map(|s| (*s).to_owned()),
                );
            }
        }
        argv
    }

    fn with(&self, extra: &[&str]) -> Vec<String> {
        let mut argv = self.argv.clone();
        argv.extend(extra.iter().map(|s| (*s).to_owned()));
        argv
    }
}

fn split_command(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

fn installed_version(package_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(package_dir.join("package.json")).ok()?;
    let at = text.find("\"version\"")?;
    let rest = &text[at + "\"version\"".len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_owned())
}

fn version_matches(wanted: &str, actual: &str) -> bool {
    actual == wanted || actual.starts_with(&format!("{wanted}."))
}

fn ensure_typescript(tools: &Path, name: &str, version: &str) -> Result<PathBuf, String> {
    let dir = tools.join(name);
    let package = dir.join("node_modules").join("typescript");
    if installed_version(&package).is_some_and(|v| version_matches(version, &v)) {
        return Ok(dir);
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    eprintln!("Installing typescript@{version} for {name}...");
    let spec = format!("typescript@{version}");
    run_visible(
        &[
            "npm",
            "install",
            "--prefix",
            &dir.to_string_lossy(),
            "--no-audit",
            "--no-fund",
            "--no-save",
            &spec,
        ],
        &dir,
    )?;
    Ok(dir)
}

/// The native tsgo binary shipped beside typescript@7, so Node's launcher is not in the timing.
fn native_tsgo(dir: &Path) -> Option<PathBuf> {
    let scope = dir.join("node_modules").join("@typescript");
    for entry in fs::read_dir(scope).ok()?.flatten() {
        let candidate = entry.path().join("lib").join("tsc");
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("typescript-")
            && candidate.is_file()
        {
            return Some(candidate);
        }
    }
    None
}

fn probe_version(argv: &[String], cwd: &Path) -> Result<String, String> {
    let mut command = argv.to_vec();
    command.push("--version".to_owned());
    let run = run_measured(&command, cwd, Duration::from_secs(60))?;
    let text =
        String::from_utf8_lossy(&run.stdout).into_owned() + &String::from_utf8_lossy(&run.stderr);
    let line = first_line(&text);
    if run.exit.code != Some(0) || line.is_empty() {
        return Err(format!("`{}` did not report a version", command.join(" ")));
    }
    Ok(line)
}

fn prepare_tools(args: &Args, root: &Path, cwd: &Path) -> Result<Vec<Tool>, String> {
    let tools_dir = root.join(".harness").join("tools");
    let mut tools = Vec::new();

    // tsc: classic JavaScript compiler.
    let mut notes = Vec::new();
    if !args.tsox_only {
        let tsc_argv = match &args.tsc {
            Some(command) => {
                notes.push("command given with --tsc or TSC_BIN".to_owned());
                split_command(command)
            }
            None if args.no_install && !tools_dir.join("tsc").exists() => {
                return Err("no tsc: pass --tsc or drop --no-install".to_owned());
            }
            None => {
                let dir = ensure_typescript(&tools_dir, "tsc", &args.tsc_version)?;
                vec![
                    dir.join("node_modules")
                        .join(".bin")
                        .join("tsc")
                        .to_string_lossy()
                        .into_owned(),
                ]
            }
        };
        let version = probe_version(&tsc_argv, cwd)?;
        tools.push(Tool {
            kind: Kind::Tsc,
            argv: tsc_argv,
            version,
            notes,
        });

        // tsgo: the native port, called directly.
        let mut notes = Vec::new();
        let tsgo_argv = match &args.tsgo {
            Some(command) => {
                notes.push("command given with --tsgo or TSGO_BIN".to_owned());
                split_command(command)
            }
            None if args.no_install && !tools_dir.join("tsgo").exists() => {
                return Err("no tsgo: pass --tsgo or drop --no-install".to_owned());
            }
            None => {
                let dir = ensure_typescript(&tools_dir, "tsgo", &args.tsgo_version)?;
                match native_tsgo(&dir) {
                    Some(binary) => {
                        notes.push("native binary, called directly (no Node launcher)".to_owned());
                        vec![binary.to_string_lossy().into_owned()]
                    }
                    None => {
                        notes.push(
                            "NOTE: native binary not found; running through the Node launcher, so \
                         timing includes Node startup"
                                .to_owned(),
                        );
                        vec![
                            dir.join("node_modules")
                                .join(".bin")
                                .join("tsc")
                                .to_string_lossy()
                                .into_owned(),
                        ]
                    }
                }
            }
        };
        let version = probe_version(&tsgo_argv, cwd)?;
        tools.push(Tool {
            kind: Kind::Tsgo,
            argv: tsgo_argv,
            version,
            notes,
        });
    }

    // tsox: the checker under test.
    let target_dir = env_text("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let default_bin = target_dir.join("release").join("tsox");
    let tsox_path = args
        .tsox
        .clone()
        .unwrap_or_else(|| default_bin.to_string_lossy().into_owned());
    if !Path::new(&tsox_path).is_file() {
        return Err(format!(
            "tsox binary not found at {tsox_path}; build it with `cargo build --release --bin tsox` \
             or pass --tsox"
        ));
    }
    let mut notes = Vec::new();
    if tsox_path.contains("/debug/") {
        notes.push(
            "WARNING: debug build. Timing and memory are NOT meaningful; use a release build"
                .to_owned(),
        );
    }
    let version = format!(
        "commit {} (binary {})",
        capture(&["git", "rev-parse", "--short", "HEAD"], root)
            .map(|sha| {
                let dirty =
                    capture(&["git", "status", "--porcelain"], root).is_some_and(|s| !s.is_empty());
                if dirty {
                    format!("{sha}, working tree has uncommitted changes")
                } else {
                    sha
                }
            })
            .unwrap_or_else(|| "unknown commit".to_owned()),
        &file_fingerprint(Path::new(&tsox_path))[..12]
    );
    tools.push(Tool {
        kind: Kind::Tsox,
        argv: vec![tsox_path],
        version,
        notes,
    });
    Ok(tools)
}

// ---------------------------------------------------------------------------------------------
// The measured runs.
// ---------------------------------------------------------------------------------------------

struct Sample {
    wall_ms: f64,
    cpu_ms: f64,
    rss_mb: Option<f64>,
    exit: Exit,
    timed_out: bool,
}

#[derive(Default)]
struct Series {
    samples: Vec<Sample>,
    /// Combined stdout and stderr of the last timed run.
    last_output: String,
    output_hashes: Vec<String>,
    spawn_error: Option<String>,
    /// The largest measurement floor over the runs (see `RawRun::floor_mb`).
    floor_mb: f64,
}

impl Series {
    fn output_stable(&self) -> bool {
        self.output_hashes.windows(2).all(|pair| pair[0] == pair[1])
    }
    fn failed(&self) -> bool {
        self.spawn_error.is_some()
            || self
                .samples
                .iter()
                .any(|s| s.timed_out || s.exit.signal.is_some())
    }
}

fn combined(run: &RawRun) -> String {
    let mut text = String::from_utf8_lossy(&run.stdout).into_owned();
    let err = String::from_utf8_lossy(&run.stderr);
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    text
}

fn save(dir: &Path, name: &str, text: &str) {
    if let Err(error) = fs::write(dir.join(name), text) {
        eprintln!("warning: could not write {name}: {error}");
    }
}

// ---------------------------------------------------------------------------------------------
// Machine information.
// ---------------------------------------------------------------------------------------------

fn load_average() -> Option<String> {
    let text = fs::read_to_string("/proc/loadavg").ok()?;
    Some(
        text.split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn machine() -> Vec<(String, String)> {
    let cwd = Path::new(".");
    let mut info = vec![
        (
            "os".to_owned(),
            format!("{} {}", env::consts::OS, env::consts::ARCH),
        ),
        (
            "kernel".to_owned(),
            capture(&["uname", "-sr"], cwd).unwrap_or_else(|| "unknown".to_owned()),
        ),
    ];
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_owned())
        })
        .or_else(|| capture(&["sysctl", "-n", "machdep.cpu.brand_string"], cwd))
        .unwrap_or_else(|| "unknown".to_owned());
    info.push(("cpu".to_owned(), cpu));
    info.push((
        "logical cores".to_owned(),
        thread::available_parallelism()
            .map_or(1, |n| n.get())
            .to_string(),
    ));
    let memory = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("MemTotal"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<f64>().ok())
                .map(|kb| format!("{:.1} GB", kb / 1024.0 / 1024.0))
        })
        .or_else(|| {
            capture(&["sysctl", "-n", "hw.memsize"], cwd)
                .and_then(|b| b.parse::<f64>().ok())
                .map(|b| format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0))
        })
        .unwrap_or_else(|| "unknown".to_owned());
    info.push(("memory".to_owned(), memory));
    info.push((
        "node".to_owned(),
        capture(&["node", "--version"], cwd).unwrap_or_else(|| "not found".to_owned()),
    ));
    info
}

// ---------------------------------------------------------------------------------------------
// The whole run.
// ---------------------------------------------------------------------------------------------

struct ToolResult {
    kind: Kind,
    series: Series,
    errors: Vec<Diag>,
    extended: BTreeMap<String, String>,
    project_files: Option<usize>,
    tsox: Option<TsoxParse>,
}

// `--tsox-only`: what tsox does on its own on the project. No reference compiler, so no claim
// about right or wrong; the numbers are speed, memory, what it reports by code, what it could not
// check, and whether two runs agree.
#[allow(clippy::too_many_arguments)]
fn tsox_only_report(
    args: &Args,
    out_dir: &Path,
    fixture: &Fixture,
    install: &str,
    machine_info: &[(String, String)],
    load: (&Option<String>, &Option<String>),
    tools: &[Tool],
    results: &[ToolResult],
    mut warnings: Vec<String>,
    mut invalid: Vec<String>,
) -> Result<u8, String> {
    let tool = tools.first().ok_or("internal: no tsox tool")?;
    let result = results.first().ok_or("internal: no tsox result")?;
    let parse = result.tsox.as_ref().ok_or("internal: no tsox parse")?;
    let series = &result.series;
    if let Some(error) = &series.spawn_error {
        invalid.push(format!("tsox: {error}"));
    }
    if series.samples.len() < args.runs && series.spawn_error.is_none() {
        invalid.push(format!(
            "tsox: only {} of {} runs finished",
            series.samples.len(),
            args.runs
        ));
    }
    if series.samples.iter().any(|s| s.timed_out) {
        invalid.push(format!(
            "tsox: a run hit the {}s timeout",
            args.timeout.as_secs()
        ));
    }
    if let Some(signal) = series.samples.iter().find_map(|s| s.exit.signal) {
        invalid.push(format!("tsox: a run was killed by signal {signal} (crash)"));
    }
    if !series.output_stable() {
        invalid.push("tsox: the output differed between runs (nondeterministic)".to_owned());
    }
    for note in &tool.notes {
        if note.starts_with("WARNING") {
            warnings.push(note.clone());
        }
    }
    let wall: Vec<f64> = series.samples.iter().map(|s| s.wall_ms).collect();
    let cpu: Vec<f64> = series.samples.iter().map(|s| s.cpu_ms).collect();
    let rss: Vec<f64> = series.samples.iter().filter_map(|s| s.rss_mb).collect();
    let mut by_code: BTreeMap<&str, usize> = BTreeMap::new();
    for diag in &parse.errors {
        *by_code.entry(diag.code.as_str()).or_default() += 1;
    }
    let mut by_file: BTreeMap<&str, usize> = BTreeMap::new();
    for diag in &parse.errors {
        *by_file.entry(diag.file.as_str()).or_default() += 1;
    }

    let mut report = String::new();
    let _ = writeln!(report, "# tsox on {} (tsox only)\n", fixture.description);
    let _ = writeln!(
        report,
        "No reference compiler ran, so this says how tsox performs and what it reports, not whether          the reports are right.
"
    );
    if invalid.is_empty() {
        report.push_str("**Valid run.**\n\n");
    } else {
        report.push_str(
            "**INVALID RUN:**
",
        );
        for reason in &invalid {
            let _ = writeln!(report, "- {reason}");
        }
        report.push('\n');
    }
    for warning in &warnings {
        let _ = writeln!(report, "- warning: {warning}");
    }
    let _ = writeln!(report, "- tsox: {}", tool.version);
    let _ = writeln!(
        report,
        "- project: {} @ {}",
        fixture.description, fixture.revision
    );
    let _ = writeln!(report, "- dependencies: {install}");
    let _ = writeln!(
        report,
        "- runs: {} timed after {} warm-up; load average {} then {}",
        args.runs,
        args.warmup,
        load.0.as_deref().unwrap_or("n/a"),
        load.1.as_deref().unwrap_or("n/a"),
    );
    for (key, value) in machine_info {
        let _ = writeln!(report, "- {key}: {value}");
    }
    report.push_str("\n## Speed and memory\n\n");
    let row = |name: &str, values: &[f64]| -> Vec<String> {
        match stats(values) {
            Some(s) => vec![
                name.to_owned(),
                f1(s.median),
                f1(s.min),
                f1(s.max),
                f1(s.mean),
                format!("{:.1}%", s.cv),
            ],
            None => vec![
                name.to_owned(),
                "n/a".into(),
                "n/a".into(),
                "n/a".into(),
                "n/a".into(),
                "n/a".into(),
            ],
        }
    };
    report.push_str(&md_table(
        &["", "median", "min", "max", "mean", "cv"],
        &[
            row("wall (ms)", &wall),
            row("cpu (ms)", &cpu),
            row("peak memory (MB)", &rss),
        ],
    ));
    report.push_str("\n## What tsox says\n\n");
    let _ = writeln!(
        report,
        "- Files checked: {}; module graph: {} file(s), {} cycle(s), {} unresolved import(s)",
        parse.checked.map_or("n/a".to_owned(), |n| n.to_string()),
        parse
            .graph_files
            .map_or("n/a".to_owned(), |n| n.to_string()),
        parse.cycles.map_or("n/a".to_owned(), |n| n.to_string()),
        parse
            .unresolved_imports
            .map_or("n/a".to_owned(), |n| n.to_string()),
    );
    let _ = writeln!(
        report,
        "- Errors reported: {} ({} in project files); warnings: {}",
        parse.reported_errors.unwrap_or(parse.errors.len()),
        parse
            .errors
            .iter()
            .filter(|d| is_project_file(&d.file))
            .count(),
        parse.reported_warnings.unwrap_or(parse.warnings.len()),
    );
    let _ = writeln!(
        report,
        "- Files tsox could only partly check: {}",
        parse.partial.len()
    );
    let _ = writeln!(
        report,
        "- Exit: {}; output identical across runs: {}",
        series
            .samples
            .last()
            .map_or("n/a".to_owned(), |s| s.exit.describe()),
        if series.output_stable() { "yes" } else { "NO" }
    );
    if !by_code.is_empty() {
        report.push_str("\n### Errors by code\n\n");
        let mut rows: Vec<(&&str, &usize)> = by_code.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|(code, count)| vec![(**code).to_owned(), count.to_string()])
            .collect();
        report.push_str(&md_table(&["code", "count"], &table));
    }
    if !by_file.is_empty() {
        report.push_str("\n### Files with the most errors\n\n");
        let mut rows: Vec<(&&str, &usize)> = by_file.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let table: Vec<Vec<String>> = rows
            .iter()
            .take(15)
            .map(|(file, count)| vec![(**file).to_owned(), count.to_string()])
            .collect();
        report.push_str(&md_table(&["file", "errors"], &table));
    }
    if !parse.partial.is_empty() {
        report.push_str("\n### Partly checked\n\n");
        for (file, reason) in parse.partial.iter().take(25) {
            let _ = writeln!(report, "- {file}: {reason}");
        }
    }
    save(out_dir, "report.md", &report);
    let summary = obj(vec![
        ("mode", jstr("tsox-only")),
        ("valid", J::Bool(invalid.is_empty())),
        ("tsox_version", jstr(&tool.version)),
        ("runs", jnum(series.samples.len() as f64)),
        (
            "wall_ms_median",
            jnum(stats(&wall).map_or(f64::NAN, |s| s.median)),
        ),
        (
            "cpu_ms_median",
            jnum(stats(&cpu).map_or(f64::NAN, |s| s.median)),
        ),
        (
            "peak_mb_median",
            jnum(stats(&rss).map_or(f64::NAN, |s| s.median)),
        ),
        ("errors", jnum(parse.errors.len() as f64)),
        ("warnings", jnum(parse.warnings.len() as f64)),
        ("checked_files", jnum(parse.checked.unwrap_or(0) as f64)),
        ("partial_files", jnum(parse.partial.len() as f64)),
        ("output_stable", J::Bool(series.output_stable())),
    ]);
    let mut text = String::new();
    summary.render(0, &mut text);
    save(out_dir, "summary.json", &text);
    println!("{report}");
    println!("results: {}", out_dir.display());
    Ok(if args.strict && !invalid.is_empty() {
        1
    } else {
        0
    })
}

fn run(args: &Args) -> Result<u8, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let started_at = now_secs();
    let out_dir = args.out.clone().unwrap_or_else(|| {
        root.join(".harness")
            .join("results")
            .join(utc_stamp(started_at))
    });
    let raw_dir = out_dir.join("raw");
    fs::create_dir_all(&raw_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let out_dir = out_dir.canonicalize().map_err(|e| e.to_string())?;
    let raw_dir = out_dir.join("raw");

    eprintln!("== tsox harness ==\nresults: {}", out_dir.display());
    let fixture = prepare_fixture(args, &root)?;
    let install = install_dependencies(&fixture, args, &root)?;
    let project_root = fixture.dir.canonicalize().map_err(|e| e.to_string())?;
    let tools = prepare_tools(args, &root, &project_root)?;
    let tsconfig_arg = fixture.tsconfig.to_string_lossy().into_owned();
    let machine_info = machine();
    let load_before = load_average();

    let mut warnings: Vec<String> = Vec::new();
    let mut invalid: Vec<String> = Vec::new();
    for tool in &tools {
        for note in &tool.notes {
            if note.starts_with("WARNING") || note.starts_with("NOTE") {
                warnings.push(format!("{}: {note}", tool.kind.name()));
            }
        }
    }
    if install.contains("UNLOCKED") {
        warnings.push(format!(
            "dependencies were installed without a lockfile ({install}); versions can differ \
             between runs"
        ));
    }

    // Timed runs: warm-up first, then rounds in which the compiler order rotates.
    let mut results: Vec<ToolResult> = tools
        .iter()
        .map(|tool| ToolResult {
            kind: tool.kind,
            series: Series::default(),
            errors: Vec::new(),
            extended: BTreeMap::new(),
            project_files: None,
            tsox: None,
        })
        .collect();
    let timeout = args.timeout;
    for round in 0..args.warmup {
        for tool in &tools {
            eprintln!("warm-up {}/{} {}", round + 1, args.warmup, tool.kind.name());
            let _ = run_measured(&tool.timed(&tsconfig_arg), &project_root, timeout);
        }
    }
    for round in 0..args.runs {
        for step in 0..tools.len() {
            let index = (step + round) % tools.len();
            let tool = &tools[index];
            eprintln!("run {}/{} {}", round + 1, args.runs, tool.kind.name());
            let series = &mut results[index].series;
            match run_measured(&tool.timed(&tsconfig_arg), &project_root, timeout) {
                Ok(run) => {
                    let text = combined(&run);
                    save(
                        &raw_dir,
                        &format!("{}.run{}.txt", tool.kind.name(), round + 1),
                        &text,
                    );
                    series.output_hashes.push(fingerprint(text.as_bytes()));
                    series.floor_mb = series.floor_mb.max(run.floor_mb);
                    series.samples.push(Sample {
                        wall_ms: run.wall_ms,
                        cpu_ms: run.usage.map_or(0.0, |u| u.user_ms + u.sys_ms),
                        rss_mb: run.usage.map(|u| u.max_rss_mb),
                        exit: run.exit,
                        timed_out: run.timed_out,
                    });
                    series.last_output = text;
                }
                Err(error) => {
                    series.spawn_error = Some(error);
                }
            }
        }
    }

    // Untimed extras: what each compiler says about its own work, and which files it saw.
    for (tool, result) in tools.iter().zip(results.iter_mut()) {
        match tool.kind {
            Kind::Tsc | Kind::Tsgo => {
                if let Ok(run) = run_measured(
                    &tool.with(&[
                        "--noEmit",
                        "--pretty",
                        "false",
                        "-p",
                        &tsconfig_arg,
                        "--extendedDiagnostics",
                    ]),
                    &project_root,
                    timeout,
                ) {
                    let text = combined(&run);
                    save(
                        &raw_dir,
                        &format!("{}.extended.txt", tool.kind.name()),
                        &text,
                    );
                    result.extended = parse_extended(&text);
                }
                if let Ok(run) = run_measured(
                    &tool.with(&["-p", &tsconfig_arg, "--listFilesOnly"]),
                    &project_root,
                    timeout,
                ) {
                    let text = combined(&run);
                    save(&raw_dir, &format!("{}.files.txt", tool.kind.name()), &text);
                    let files: BTreeSet<String> = text
                        .lines()
                        .map(|l| rel_path(&project_root, l.trim()))
                        .filter(|l| is_project_file(l))
                        .collect();
                    result.project_files = Some(files.len());
                }
            }
            Kind::Tsox => {
                if let Ok(run) = run_measured(
                    &tool.with(&["--project", &tsconfig_arg, "--tsc-codes", "--partial"]),
                    &project_root,
                    timeout,
                ) {
                    let text = combined(&run);
                    save(&raw_dir, "tsox.partial.txt", &text);
                    result.tsox = Some(parse_tsox_output(&text, &project_root));
                }
            }
        }
    }
    let load_after = load_average();
    let mut timings = String::from("compiler,run,wall_ms,cpu_ms,peak_rss_mb,exit,timed_out\n");
    for result in &results {
        for (index, sample) in result.series.samples.iter().enumerate() {
            let _ = writeln!(
                timings,
                "{},{},{:.3},{:.3},{},{},{}",
                result.kind.name(),
                index + 1,
                sample.wall_ms,
                sample.cpu_ms,
                sample.rss_mb.map_or(String::new(), |m| format!("{m:.3}")),
                sample.exit.describe(),
                sample.timed_out
            );
        }
    }
    save(&raw_dir, "timings.csv", &timings);

    // Parse the diagnostics of the last timed run of each compiler.
    for result in &mut results {
        match result.kind {
            Kind::Tsc | Kind::Tsgo => {
                result.errors = parse_ts_output(&result.series.last_output, &project_root);
            }
            Kind::Tsox => {
                let parsed = parse_tsox_output(&result.series.last_output, &project_root);
                result.errors = parsed.errors.clone();
                let partial = result.tsox.take().map(|p| p.partial).unwrap_or_default();
                result.tsox = Some(TsoxParse { partial, ..parsed });
            }
        }
    }

    if args.tsox_only {
        return tsox_only_report(
            args,
            &out_dir,
            &fixture,
            &install,
            &machine_info,
            (&load_before, &load_after),
            &tools,
            &results,
            warnings,
            invalid,
        );
    }

    let by_kind = |kind: Kind| results.iter().find(|r| r.kind == kind);
    let tsc = by_kind(Kind::Tsc).ok_or("internal: no tsc result")?;
    let tsgo = by_kind(Kind::Tsgo).ok_or("internal: no tsgo result")?;
    let tsox = by_kind(Kind::Tsox).ok_or("internal: no tsox result")?;
    let tsox_parse = tsox.tsox.as_ref().ok_or("internal: no tsox parse")?;

    // Validity: could the numbers mean anything?
    for result in &results {
        let name = result.kind.name();
        if let Some(error) = &result.series.spawn_error {
            invalid.push(format!("{name}: {error}"));
        }
        if result.series.samples.len() < args.runs && result.series.spawn_error.is_none() {
            invalid.push(format!(
                "{name}: only {} of {} runs finished",
                result.series.samples.len(),
                args.runs
            ));
        }
        if result.series.samples.iter().any(|s| s.timed_out) {
            invalid.push(format!(
                "{name}: a run hit the {}s timeout",
                args.timeout.as_secs()
            ));
        }
        if let Some(signal) = result.series.samples.iter().find_map(|s| s.exit.signal) {
            invalid.push(format!(
                "{name}: a run was killed by signal {signal} (crash)"
            ));
        }
        if !result.series.output_stable() {
            warnings.push(format!(
                "{name}: the output differed between runs (nondeterministic output)"
            ));
        }
    }
    for reference in [tsc, tsgo] {
        let broken: Vec<&Diag> = reference
            .errors
            .iter()
            .filter(|d| d.code == "TS2688" || d.code == "TS2307" || d.code == "TS2792")
            .collect();
        if !broken.is_empty() {
            invalid.push(format!(
                "{}: {} 'cannot find module or type definition' errors (TS2688/TS2307/TS2792), so \
                 dependencies are probably not installed and every number below is suspect; first: {}",
                reference.kind.name(),
                broken.len(),
                one_line(&format!("{} {}", broken[0].code, broken[0].message))
            ));
        }
    }
    for result in &results {
        if let Some(m) = stats(
            &result
                .series
                .samples
                .iter()
                .filter_map(|s| s.rss_mb)
                .collect::<Vec<_>>(),
        ) {
            if m.median < result.series.floor_mb * 1.5 {
                warnings.push(format!(
                    "{}: peak memory {} MB is within 1.5x of the measuring process's own {} MB, so it is not reliable",
                    result.kind.name(),
                    f1(m.median),
                    f1(result.series.floor_mb)
                ));
            }
        }
    }
    if let (Some(a), Some(b)) = (tsc.project_files, tsgo.project_files) {
        if a != b {
            warnings.push(format!("tsc sees {a} project files and tsgo sees {b}"));
        }
    }
    if let (Some(reference), Some(checked)) = (tsc.project_files, tsox_parse.checked) {
        if reference != checked {
            warnings.push(format!(
                "scope differs: tsc sees {reference} project files, tsox checked {checked}; the \
                 comparison covers only what both looked at, and a smaller scope is not a speed win"
            ));
        }
    }
    if tsox.errors.len() != tsox_parse.reported_errors.unwrap_or(tsox.errors.len()) {
        warnings.push(format!(
            "parsed {} tsox errors but tsox reports {}; the output format may have changed",
            tsox.errors.len(),
            tsox_parse.reported_errors.unwrap_or(0)
        ));
    }

    // Agreement.
    let project_only = |diags: &[Diag]| -> Vec<Diag> {
        diags
            .iter()
            .filter(|d| is_project_file(&d.file))
            .cloned()
            .collect()
    };
    let tsc_errors = project_only(&tsc.errors);
    let tsgo_errors = project_only(&tsgo.errors);
    let tsox_errors = project_only(&tsox.errors);
    let outside = |all: &[Diag], inside: &[Diag]| all.len() - inside.len();
    let vs_tsc = compare(&tsc_errors, &tsox_errors);
    let vs_tsgo = compare(&tsgo_errors, &tsox_errors);
    let tsc_vs_tsgo = compare(&tsc_errors, &tsgo_errors);

    // Per-file classification against tsc, over every file any compiler reported on or tsc saw.
    let mut files: BTreeSet<String> = BTreeSet::new();
    for diag in tsc_errors.iter().chain(&tsgo_errors).chain(&tsox_errors) {
        files.insert(diag.file.clone());
    }
    let tsc_listing = fs::read_to_string(raw_dir.join("tsc.files.txt")).unwrap_or_default();
    for line in tsc_listing.lines() {
        let relative = rel_path(&project_root, line.trim());
        if is_project_file(&relative) {
            files.insert(relative);
        }
    }
    let warnings_by_file: BTreeMap<String, usize> = {
        let mut map = BTreeMap::new();
        for diag in &tsox_parse.warnings {
            *map.entry(diag.file.clone()).or_insert(0) += 1;
        }
        map
    };
    let partial_files: BTreeSet<&str> =
        tsox_parse.partial.iter().map(|(f, _)| f.as_str()).collect();
    let mut class_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    // Agreeing files where tsox skipped nothing: the agreement that counts for most.
    let mut agree_checked = 0usize;
    let mut csv = String::from(
        "file,class,tsc_errors,tsgo_errors,tsox_errors,false_positives,missed,tsox_unchecked_markers,tsox_partial_exports\n",
    );
    let mut per_file_rows: Vec<(String, FileClass, usize, usize, usize, usize, usize)> = Vec::new();
    for file in &files {
        let (a, g, x) = (
            in_file(&tsc_errors, file),
            in_file(&tsgo_errors, file),
            in_file(&tsox_errors, file),
        );
        let class = classify(&a, &x);
        *class_counts.entry(class.label()).or_insert(0) += 1;
        let one_file = compare(
            &a.iter().map(|d| (*d).clone()).collect::<Vec<_>>(),
            &x.iter().map(|d| (*d).clone()).collect::<Vec<_>>(),
        );
        let marks = warnings_by_file.get(file).copied().unwrap_or(0);
        let partial = usize::from(partial_files.contains(file.as_str()));
        if class == FileClass::Agree && marks == 0 && partial == 0 {
            agree_checked += 1;
        }
        let _ = writeln!(
            csv,
            "{file},{},{},{},{},{},{},{marks},{partial}",
            class.label(),
            a.len(),
            g.len(),
            x.len(),
            one_file.false_positive_total(),
            one_file.missed_total()
        );
        per_file_rows.push((
            file.clone(),
            class,
            a.len(),
            x.len(),
            one_file.false_positive_total(),
            one_file.missed_total(),
            marks,
        ));
    }
    save(&out_dir, "per-file.csv", &csv);

    let describe_key = |key: &Key, count: usize, source: &[Diag]| -> String {
        let message = source
            .iter()
            .find(|d| d.file == key.0 && d.line == key.1 && d.code == key.2)
            .map_or(String::new(), |d| one_line(&d.message));
        let times = if count > 1 {
            format!(" (x{count})")
        } else {
            String::new()
        };
        format!("{}:{} {}{times} {message}", key.0, key.1, key.2)
    };
    let mut fp_text = String::new();
    for (key, count) in &vs_tsc.false_positives {
        let _ = writeln!(fp_text, "{}", describe_key(key, *count, &tsox_errors));
    }
    let mut missed_text = String::new();
    for (key, count) in &vs_tsc.missed {
        let _ = writeln!(missed_text, "{}", describe_key(key, *count, &tsc_errors));
    }
    save(&out_dir, "tsox-false-positives.txt", &fp_text);
    save(&out_dir, "tsox-missed.txt", &missed_text);
    let mut ref_diff = String::new();
    for (key, count) in &tsc_vs_tsgo.false_positives {
        let _ = writeln!(
            ref_diff,
            "only tsgo: {}",
            describe_key(key, *count, &tsgo_errors)
        );
    }
    for (key, count) in &tsc_vs_tsgo.missed {
        let _ = writeln!(
            ref_diff,
            "only tsc:  {}",
            describe_key(key, *count, &tsc_errors)
        );
    }
    save(&out_dir, "tsc-vs-tsgo-differences.txt", &ref_diff);

    // Codes: which false positives and gaps dominate.
    let by_code = |items: &[(Key, usize)], source: &[Diag]| -> Vec<(String, usize, String)> {
        let mut counts: BTreeMap<String, (usize, String)> = BTreeMap::new();
        for (key, count) in items {
            let entry = counts.entry(key.2.clone()).or_insert_with(|| {
                let message = source
                    .iter()
                    .find(|d| d.code == key.2)
                    .map_or(String::new(), |d| one_line(&d.message));
                (0, message)
            });
            entry.0 += count;
        }
        let mut rows: Vec<(String, usize, String)> =
            counts.into_iter().map(|(c, (n, m))| (c, n, m)).collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        rows
    };
    let fp_codes = by_code(&vs_tsc.false_positives, &tsox_errors);
    let missed_codes = by_code(&vs_tsc.missed, &tsc_errors);

    // tsox coverage markers.
    let mut marker_codes: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for diag in &tsox_parse.warnings {
        let entry = marker_codes
            .entry(diag.code.clone())
            .or_insert_with(|| (0, one_line(&diag.message)));
        entry.0 += 1;
    }
    let mut marker_rows: Vec<(String, usize, String)> = marker_codes
        .into_iter()
        .map(|(c, (n, m))| (c, n, m))
        .collect();
    marker_rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut partial_reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, reason) in &tsox_parse.partial {
        *partial_reasons.entry(reason.as_str()).or_insert(0) += 1;
    }

    // Optional allocation profile of the benchmark workloads.
    let mut dhat_text: Option<String> = None;
    if args.dhat {
        eprintln!("Running examples/dhat_heap (allocation profile)...");
        // Extra cargo flags, for example `--ignore-rust-version` on an older toolchain.
        let extra = env_text("HARNESS_CARGO_FLAGS").unwrap_or_default();
        let mut command = Command::new("cargo");
        command
            .arg("run")
            .arg("--release")
            .arg("--manifest-path")
            .arg(root.join("Cargo.toml"))
            .arg("--example")
            .arg("dhat_heap")
            .arg("--features")
            .arg("dhat-heap")
            .args(extra.split_whitespace())
            // dhat writes dhat-heap.json into the working directory: keep it with the results.
            .current_dir(&raw_dir)
            .env("CARGO_PROFILE_RELEASE_DEBUG", "true")
            .stdin(Stdio::null());
        match command.output() {
            Ok(output) => {
                save(
                    &raw_dir,
                    "dhat_heap.err.txt",
                    &String::from_utf8_lossy(&output.stderr),
                );
                if output.status.success() {
                    let text = String::from_utf8_lossy(&output.stdout).into_owned();
                    save(&raw_dir, "dhat_heap.txt", &text);
                    dhat_text = Some(text);
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let tail: Vec<&str> = stderr
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .rev()
                        .take(2)
                        .collect();
                    warnings.push(format!(
                        "examples/dhat_heap failed ({}): {}; full cargo output in raw/dhat_heap.err.txt",
                        output.status,
                        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
                    ));
                }
            }
            Err(error) => warnings.push(format!("could not run examples/dhat_heap: {error}")),
        }
    }

    // ---- statistics and the report ----
    let wall =
        |r: &ToolResult| -> Vec<f64> { r.series.samples.iter().map(|s| s.wall_ms).collect() };
    let cpu = |r: &ToolResult| -> Vec<f64> { r.series.samples.iter().map(|s| s.cpu_ms).collect() };
    let rss =
        |r: &ToolResult| -> Vec<f64> { r.series.samples.iter().filter_map(|s| s.rss_mb).collect() };
    let tsox_wall = stats(&wall(tsox));
    let tsox_rss = stats(&rss(tsox));

    let mut report = String::new();
    let verdict = if invalid.is_empty() {
        "VALID"
    } else {
        "INVALID"
    };
    let _ = writeln!(report, "# tsox harness report\n");
    let _ = writeln!(
        report,
        "**{}** | {} | {verdict}\n",
        utc_pretty(started_at),
        fixture.description
    );
    if !invalid.is_empty() {
        let _ = writeln!(report, "## This run is not trustworthy\n");
        for reason in &invalid {
            let _ = writeln!(report, "- {reason}");
        }
        let _ = writeln!(report);
    }
    if !warnings.is_empty() {
        let _ = writeln!(report, "## Warnings\n");
        for warning in &warnings {
            let _ = writeln!(report, "- {warning}");
        }
        let _ = writeln!(report);
    }

    // Headline.
    let _ = writeln!(report, "## Summary\n");
    let class_total: usize = class_counts.values().sum();
    let count = |label: &str| class_counts.get(label).copied().unwrap_or(0);
    let _ = writeln!(
        report,
        "- Files compared (against tsc): **{class_total}**: {} agree ({agree_checked} of them with nothing skipped by tsox), {} agree on lines only, {} gap, {} false positive, {} mixed",
        count("agree"),
        count("agree-lines"),
        count("gap"),
        count("false-positive"),
        count("mixed")
    );
    let _ = writeln!(
        report,
        "- Errors: tsc {} | tsgo {} | tsox {}. Matched {} (precision {}, recall {}). **{} false positives, {} missed.**",
        tsc_errors.len(),
        tsgo_errors.len(),
        tsox_errors.len(),
        vs_tsc.matched,
        pct(vs_tsc.precision()),
        pct(vs_tsc.recall()),
        vs_tsc.false_positive_total(),
        vs_tsc.missed_total()
    );
    if let (Some(t), Some(x)) = (stats(&wall(tsc)), tsox_wall) {
        let _ = writeln!(
            report,
            "- Speed (median wall): tsc {} ms | tsgo {} ms | tsox {} ms",
            f1(t.median),
            stats(&wall(tsgo)).map_or("n/a".to_owned(), |s| f1(s.median)),
            f1(x.median)
        );
    }
    let _ = writeln!(
        report,
        "- tsox did not fully check {} file(s) (partial exports) and left {} construct(s) unchecked (TSR9xxx markers).\n",
        tsox_parse.partial.len(),
        tsox_parse.warnings.len()
    );
    let _ = writeln!(
        report,
        "> How to read this: agreement is by file, line and tsc error code, with tsc as the reference. \
         It is a compatibility signal, not proof of equal semantics. tsox checks a smaller language, so \
         a speed ratio only means something next to these agreement numbers. Every figure here can be \
         recomputed from the files in this folder (see REPRODUCE.sh).\n"
    );

    // Speed.
    let _ = writeln!(report, "## Speed\n");
    let _ = writeln!(
        report,
        "{} timed runs per compiler after {} warm-up run(s), interleaved. Wall time in milliseconds; \
         it includes process start (Node for tsc).\n",
        args.runs, args.warmup
    );
    let mut rows = Vec::new();
    for result in &results {
        if let Some(s) = stats(&wall(result)) {
            rows.push(vec![
                result.kind.name().to_owned(),
                s.n.to_string(),
                f1(s.median),
                f1(s.mean),
                f1(s.stdev),
                f1(s.min),
                f1(s.max),
                format!("{}-{}", f1(s.q1), f1(s.q3)),
                format!("{:.1}%", s.cv),
                if s.cv > 10.0 && s.n > 1 {
                    "noisy".to_owned()
                } else {
                    "steady".to_owned()
                },
            ]);
        }
    }
    report.push_str(&md_table(
        &[
            "compiler", "runs", "median", "mean", "stdev", "min", "max", "q1-q3", "cv", "noise",
        ],
        &rows,
    ));
    if let Some(x) = tsox_wall {
        let mut ratio_rows = Vec::new();
        for result in [tsc, tsgo] {
            if let Some(s) = stats(&wall(result)) {
                let by_median = s.median / x.median;
                let by_min = s.min / x.min;
                let phrase = |ratio: f64| {
                    if ratio >= 1.0 {
                        format!("tsox {ratio:.2}x faster")
                    } else {
                        format!("tsox {:.2}x slower", 1.0 / ratio)
                    }
                };
                ratio_rows.push(vec![
                    format!("{} vs tsox", result.kind.name()),
                    phrase(by_median),
                    phrase(by_min),
                ]);
            }
        }
        let _ = writeln!(
            report,
            "\n{}",
            md_table(&["pair", "by median", "by best run"], &ratio_rows)
        );
    }
    if let Some(note) = tools
        .iter()
        .find(|t| t.kind == Kind::Tsox)
        .and_then(|t| t.notes.iter().find(|n| n.starts_with("WARNING")))
    {
        let _ = writeln!(report, "**{note}**\n");
    }

    // Memory and CPU.
    let _ = writeln!(report, "## Memory and CPU\n");
    let memory_floor = results
        .iter()
        .map(|r| r.series.floor_mb)
        .fold(0.0_f64, f64::max);
    let _ = writeln!(
        report,
        "Peak resident memory of the whole process, and user plus system CPU time, from `wait4`. \
         Each program is started by a small measuring process, so the harness's own size is not \
         counted as the program's. That measuring process peaks at {} MB when it starts the \
         program; a figure close to that is not trustworthy.\n",
        f1(memory_floor)
    );
    let mut rows = Vec::new();
    for result in &results {
        if let (Some(m), Some(c), Some(w)) = (
            stats(&rss(result)),
            stats(&cpu(result)),
            stats(&wall(result)),
        ) {
            rows.push(vec![
                result.kind.name().to_owned(),
                f1(m.median),
                f1(m.max),
                f1(c.median),
                format!("{:.2}", c.median / w.median),
            ]);
        }
    }
    report.push_str(&md_table(
        &[
            "compiler",
            "peak RSS median (MB)",
            "peak RSS max (MB)",
            "CPU median (ms)",
            "CPU / wall",
        ],
        &rows,
    ));
    if let Some(x) = tsox_rss {
        for result in [tsc, tsgo] {
            if let Some(m) = stats(&rss(result)) {
                let _ = writeln!(
                    report,
                    "\n- {} uses {:.2}x the peak memory of tsox.",
                    result.kind.name(),
                    m.median / x.median
                );
            }
        }
    }
    let _ = writeln!(report);

    // Scope and the compilers' own accounting.
    let _ = writeln!(report, "## Scope and compiler-reported work\n");
    let mut rows = vec![
        vec![
            "project files seen".to_owned(),
            tsc.project_files
                .map_or("n/a".to_owned(), |n| n.to_string()),
            tsgo.project_files
                .map_or("n/a".to_owned(), |n| n.to_string()),
            tsox_parse
                .checked
                .map_or("n/a".to_owned(), |n| n.to_string()),
        ],
        vec![
            "files in tsox module graph".to_owned(),
            String::new(),
            String::new(),
            tsox_parse
                .graph_files
                .map_or("n/a".to_owned(), |n| n.to_string()),
        ],
        vec![
            "unresolved imports (tsox)".to_owned(),
            String::new(),
            String::new(),
            tsox_parse
                .unresolved_imports
                .map_or("n/a".to_owned(), |n| n.to_string()),
        ],
        vec![
            "import cycles (tsox)".to_owned(),
            String::new(),
            String::new(),
            tsox_parse
                .cycles
                .map_or("n/a".to_owned(), |n| n.to_string()),
        ],
    ];
    let keys: BTreeSet<&String> = tsc.extended.keys().chain(tsgo.extended.keys()).collect();
    for key in keys {
        rows.push(vec![
            key.clone(),
            tsc.extended.get(key).cloned().unwrap_or_default(),
            tsgo.extended.get(key).cloned().unwrap_or_default(),
            String::new(),
        ]);
    }
    report.push_str(&md_table(&["", "tsc", "tsgo", "tsox"], &rows));
    let _ = writeln!(
        report,
        "\nWork counts such as lines and symbols include the standard library, which tsox does not model the same way, so compare them only between tsc and tsgo.\n"
    );

    // Agreement in detail.
    let _ = writeln!(report, "## Agreement in detail\n");
    let mut rows = Vec::new();
    for (label, c) in [
        ("tsox vs tsc (reference)", &vs_tsc),
        ("tsox vs tsgo", &vs_tsgo),
        ("tsgo vs tsc (reference check)", &tsc_vs_tsgo),
    ] {
        rows.push(vec![
            label.to_owned(),
            c.reference_total.to_string(),
            c.candidate_total.to_string(),
            c.matched.to_string(),
            c.false_positive_total().to_string(),
            c.missed_total().to_string(),
            pct(c.precision()),
            pct(c.recall()),
            c.lines_both.to_string(),
            c.lines_reference_only.to_string(),
            c.lines_candidate_only.to_string(),
            c.lines_code_differs.to_string(),
        ]);
    }
    report.push_str(&md_table(
        &[
            "pair",
            "ref errors",
            "cand errors",
            "matched",
            "false pos",
            "missed",
            "precision",
            "recall",
            "lines both",
            "lines ref only",
            "lines cand only",
            "lines code differs",
        ],
        &rows,
    ));
    let _ = writeln!(
        report,
        "\nAn error matches when file, line and tsc code are equal. **false pos** are errors only the \
         candidate reports, **missed** are errors only the reference reports. Precision and recall are \
         n/a when a side has no errors. Errors outside the project (dependencies, libraries) were \
         left out: tsc {}, tsgo {}, tsox {}.\n",
        outside(&tsc.errors, &tsc_errors),
        outside(&tsgo.errors, &tsgo_errors),
        outside(&tsox.errors, &tsox_errors)
    );
    if tsc_vs_tsgo.false_positive_total() + tsc_vs_tsgo.missed_total() > 0 {
        let _ = writeln!(
            report,
            "**tsc and tsgo disagree on {} error(s)**, so the reference is itself uncertain; see \
             tsc-vs-tsgo-differences.txt.\n",
            tsc_vs_tsgo.false_positive_total() + tsc_vs_tsgo.missed_total()
        );
    } else {
        let _ = writeln!(
            report,
            "tsc and tsgo report the same errors, so the reference is consistent.\n"
        );
    }

    let _ = writeln!(report, "### Files by class (against tsc)\n");
    let mut rows = Vec::new();
    for label in ["agree", "agree-lines", "gap", "false-positive", "mixed"] {
        rows.push(vec![label.to_owned(), count(label).to_string()]);
    }
    report.push_str(&md_table(&["class", "files"], &rows));
    let _ = writeln!(
        report,
        "\n`agree` includes files with no errors on either side. `agree-lines`: same lines, different \
         codes or counts. `gap`: tsc reports lines tsox does not. `false-positive`: tsox reports lines \
         tsc does not. `mixed`: both.\n"
    );

    let mut shown: Vec<_> = per_file_rows
        .iter()
        .filter(|row| row.1 != FileClass::Agree || row.6 > 0)
        .collect();
    shown.sort_by(|a, b| {
        (b.4 + b.5)
            .cmp(&(a.4 + a.5))
            .then(b.6.cmp(&a.6))
            .then(a.0.cmp(&b.0))
    });
    if !shown.is_empty() {
        let scope = if shown.len() > 25 {
            format!("top 25 of {}", shown.len())
        } else {
            format!("all {}", shown.len())
        };
        let _ = writeln!(report, "### Files that need attention ({scope})\n");
        let rows: Vec<Vec<String>> = shown
            .iter()
            .take(25)
            .map(|row| {
                vec![
                    row.0.clone(),
                    row.1.label().to_owned(),
                    row.2.to_string(),
                    row.3.to_string(),
                    row.4.to_string(),
                    row.5.to_string(),
                    row.6.to_string(),
                ]
            })
            .collect();
        report.push_str(&md_table(
            &[
                "file",
                "class",
                "tsc errors",
                "tsox errors",
                "false pos",
                "missed",
                "unchecked markers",
            ],
            &rows,
        ));
        let _ = writeln!(report);
    }

    if !fp_codes.is_empty() {
        let _ = writeln!(report, "### tsox false positives by code\n");
        let rows: Vec<Vec<String>> = fp_codes
            .iter()
            .take(15)
            .map(|(c, n, m)| vec![c.clone(), n.to_string(), m.clone()])
            .collect();
        report.push_str(&md_table(&["code", "count", "example message"], &rows));
        let _ = writeln!(report, "\nFull list: tsox-false-positives.txt\n");
    }
    if !missed_codes.is_empty() {
        let _ = writeln!(report, "### Errors tsox misses, by code (gaps)\n");
        let rows: Vec<Vec<String>> = missed_codes
            .iter()
            .take(15)
            .map(|(c, n, m)| vec![c.clone(), n.to_string(), m.clone()])
            .collect();
        report.push_str(&md_table(&["code", "count", "example message"], &rows));
        let _ = writeln!(report, "\nFull list: tsox-missed.txt\n");
    }

    // Coverage.
    let _ = writeln!(report, "## What tsox did not check\n");
    if marker_rows.is_empty() && tsox_parse.partial.is_empty() {
        let _ = writeln!(report, "Nothing was flagged as unchecked.\n");
    } else {
        if !marker_rows.is_empty() {
            let _ = writeln!(
                report,
                "Each `TSR9xxx` warning marks a construct tsox skipped. A project with many of these is mostly unchecked, so agreement on it is weaker evidence.\n"
            );
            let rows: Vec<Vec<String>> = marker_rows
                .iter()
                .take(15)
                .map(|(c, n, m)| vec![c.clone(), n.to_string(), m.clone()])
                .collect();
            report.push_str(&md_table(&["marker", "count", "message"], &rows));
            let _ = writeln!(report);
        }
        if !tsox_parse.partial.is_empty() {
            let _ = writeln!(
                report,
                "{} file(s) have partial exports (from `tsox --partial`):\n",
                tsox_parse.partial.len()
            );
            let rows: Vec<Vec<String>> = partial_reasons
                .iter()
                .map(|(reason, n)| vec![(*reason).to_owned(), n.to_string()])
                .collect();
            report.push_str(&md_table(&["reason", "files"], &rows));
            let _ = writeln!(report);
        }
    }

    // Exit codes and determinism.
    let _ = writeln!(report, "## Exit codes and determinism\n");
    let mut rows = Vec::new();
    for result in &results {
        let mut codes: Vec<String> = result
            .series
            .samples
            .iter()
            .map(|s| s.exit.describe())
            .collect();
        codes.sort();
        codes.dedup();
        rows.push(vec![
            result.kind.name().to_owned(),
            codes.join(", "),
            if result.series.output_stable() {
                "yes".to_owned()
            } else {
                "NO".to_owned()
            },
        ]);
    }
    report.push_str(&md_table(
        &["compiler", "exit codes seen", "same output every run"],
        &rows,
    ));
    let _ = writeln!(
        report,
        "\ntsc and tsgo use different nonzero codes for \"errors found\" (for example 2 and 1), so the harness compares the errors, not the codes. tsox exits 1 for type errors and 2 when it cannot read the project.\n"
    );

    // Environment.
    let _ = writeln!(report, "## Environment and inputs\n");
    let mut rows: Vec<Vec<String>> = machine_info
        .iter()
        .map(|(k, v)| vec![k.clone(), v.clone()])
        .collect();
    rows.push(vec![
        "load average before / after".to_owned(),
        format!(
            "{} / {}",
            load_before.unwrap_or_else(|| "n/a".to_owned()),
            load_after.unwrap_or_else(|| "n/a".to_owned())
        ),
    ]);
    rows.push(vec!["project".to_owned(), fixture.description.clone()]);
    rows.push(vec![
        "project revision".to_owned(),
        fixture.revision.clone(),
    ]);
    rows.push(vec![
        "tsconfig".to_owned(),
        format!(
            "{} (xxh3 {})",
            fixture.tsconfig.display(),
            &file_fingerprint(&fixture.tsconfig)[..12]
        ),
    ]);
    rows.push(vec!["dependencies".to_owned(), install.clone()]);
    for tool in &tools {
        rows.push(vec![
            format!("{} version", tool.kind.name()),
            tool.version.clone(),
        ]);
        rows.push(vec![
            format!("{} command", tool.kind.name()),
            tool.timed(&tsconfig_arg).join(" "),
        ]);
    }
    report.push_str(&md_table(&["item", "value"], &rows));
    let _ = writeln!(
        report,
        "\ntsox is run with `--tsc-codes` so its errors carry tsc codes; that only changes how they print.\n"
    );

    if let Some(text) = &dhat_text {
        let _ = writeln!(report, "## Allocation profile (examples/dhat_heap)\n");
        let _ = writeln!(
            report,
            "Synthetic benchmark workloads, not this project. dhat slows the run, so use the times only to compare workloads with each other; criterion (`cargo bench`) is the place for speed.\n\n```text\n{}\n```\n",
            text.trim_end()
        );
    }

    let _ = writeln!(report, "## Files in this folder\n");
    let _ = writeln!(
        report,
        "- `summary.json`: every number above, machine readable\n- `per-file.csv`: one row per file\n- `tsox-false-positives.txt`, `tsox-missed.txt`, `tsc-vs-tsgo-differences.txt`\n- `raw/`: output of every run, and `raw/timings.csv` with every run's time, CPU and memory\n- `REPRODUCE.sh`: the exact commands"
    );
    save(&out_dir, "report.md", &report);

    // summary.json
    let tool_json = |result: &ToolResult, tool: &Tool| -> J {
        let w = stats(&wall(result));
        let stat = |s: Option<Stats>| -> J {
            match s {
                Some(s) => obj(vec![
                    ("n", jnum(s.n as f64)),
                    ("median", jnum(s.median)),
                    ("mean", jnum(s.mean)),
                    ("stdev", jnum(s.stdev)),
                    ("min", jnum(s.min)),
                    ("max", jnum(s.max)),
                    ("q1", jnum(s.q1)),
                    ("q3", jnum(s.q3)),
                    ("cv_percent", jnum(s.cv)),
                ]),
                None => J::Null,
            }
        };
        obj(vec![
            ("name", jstr(result.kind.name())),
            ("version", jstr(tool.version.clone())),
            ("command", jstr(tool.timed(&tsconfig_arg).join(" "))),
            ("wall_ms", stat(w)),
            (
                "samples",
                J::Arr(
                    result
                        .series
                        .samples
                        .iter()
                        .map(|s| {
                            obj(vec![
                                ("wall_ms", jnum(s.wall_ms)),
                                ("cpu_ms", jnum(s.cpu_ms)),
                                ("peak_rss_mb", jopt(s.rss_mb)),
                                ("exit", jstr(s.exit.describe())),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("cpu_ms", stat(stats(&cpu(result)))),
            ("peak_rss_mb", stat(stats(&rss(result)))),
            ("memory_floor_mb", jnum(result.series.floor_mb)),
            (
                "exit_codes",
                J::Arr(
                    result
                        .series
                        .samples
                        .iter()
                        .map(|s| jstr(s.exit.describe()))
                        .collect(),
                ),
            ),
            ("output_stable", J::Bool(result.series.output_stable())),
            (
                "project_files_seen",
                jopt(match result.kind {
                    Kind::Tsox => tsox_parse.checked.map(|n| n as f64),
                    _ => result.project_files.map(|n| n as f64),
                }),
            ),
            (
                "errors_in_project",
                jnum(match result.kind {
                    Kind::Tsc => tsc_errors.len(),
                    Kind::Tsgo => tsgo_errors.len(),
                    Kind::Tsox => tsox_errors.len(),
                } as f64),
            ),
        ])
    };
    let cmp_json = |c: &Comparison| -> J {
        obj(vec![
            ("reference_errors", jnum(c.reference_total as f64)),
            ("candidate_errors", jnum(c.candidate_total as f64)),
            ("matched", jnum(c.matched as f64)),
            ("false_positives", jnum(c.false_positive_total() as f64)),
            ("missed", jnum(c.missed_total() as f64)),
            ("precision", jopt(c.precision())),
            ("recall", jopt(c.recall())),
            ("lines_both", jnum(c.lines_both as f64)),
            ("lines_reference_only", jnum(c.lines_reference_only as f64)),
            ("lines_candidate_only", jnum(c.lines_candidate_only as f64)),
            ("lines_code_differs", jnum(c.lines_code_differs as f64)),
        ])
    };
    let summary = obj(vec![
        ("schema", jnum(2.0)),
        ("generated_utc", jstr(utc_pretty(started_at))),
        ("valid", J::Bool(invalid.is_empty())),
        (
            "invalid_reasons",
            J::Arr(invalid.iter().map(|s| jstr(s.clone())).collect()),
        ),
        (
            "warnings",
            J::Arr(warnings.iter().map(|s| jstr(s.clone())).collect()),
        ),
        (
            "project",
            obj(vec![
                ("description", jstr(fixture.description.clone())),
                ("revision", jstr(fixture.revision.clone())),
                ("dependencies", jstr(install.clone())),
                ("tsconfig_xxh3", jstr(file_fingerprint(&fixture.tsconfig))),
            ]),
        ),
        (
            "machine",
            J::Obj(
                machine_info
                    .iter()
                    .map(|(k, v)| (k.clone(), jstr(v.clone())))
                    .collect(),
            ),
        ),
        ("runs", jnum(args.runs as f64)),
        ("warmup", jnum(args.warmup as f64)),
        (
            "tools",
            J::Arr(
                tools
                    .iter()
                    .zip(results.iter())
                    .map(|(t, r)| tool_json(r, t))
                    .collect(),
            ),
        ),
        (
            "agreement",
            obj(vec![
                ("tsox_vs_tsc", cmp_json(&vs_tsc)),
                ("tsox_vs_tsgo", cmp_json(&vs_tsgo)),
                ("tsgo_vs_tsc", cmp_json(&tsc_vs_tsgo)),
                (
                    "files",
                    obj(vec![
                        ("compared", jnum(class_total as f64)),
                        ("agree", jnum(count("agree") as f64)),
                        ("agree_lines", jnum(count("agree-lines") as f64)),
                        ("gap", jnum(count("gap") as f64)),
                        ("false_positive", jnum(count("false-positive") as f64)),
                        ("mixed", jnum(count("mixed") as f64)),
                        ("agree_fully_checked", jnum(agree_checked as f64)),
                    ]),
                ),
            ]),
        ),
        (
            "coverage",
            obj(vec![
                ("unchecked_markers", jnum(tsox_parse.warnings.len() as f64)),
                (
                    "partial_export_files",
                    jnum(tsox_parse.partial.len() as f64),
                ),
                (
                    "unresolved_imports",
                    jopt(tsox_parse.unresolved_imports.map(|n| n as f64)),
                ),
            ]),
        ),
    ]);
    let mut text = String::new();
    summary.render(0, &mut text);
    text.push('\n');
    save(&out_dir, "summary.json", &text);

    // REPRODUCE.sh
    let mut script = String::from(
        "#!/usr/bin/env bash\n# The commands behind report.md. Run them from the project directory.\nset -u\n",
    );
    let _ = writeln!(script, "cd '{}'", project_root.display());
    for tool in &tools {
        let _ = writeln!(script, "\n# {} {}", tool.kind.name(), tool.version);
        let _ = writeln!(script, "{}", tool.timed(&tsconfig_arg).join(" "));
        if tool.kind == Kind::Tsox {
            let _ = writeln!(script, "{} --partial", tool.timed(&tsconfig_arg).join(" "));
        } else {
            let _ = writeln!(
                script,
                "{} --extendedDiagnostics",
                tool.timed(&tsconfig_arg).join(" ")
            );
        }
    }
    let _ = writeln!(
        script,
        "\n# Benchmarks and allocation profile (run from the tsox repository):\n# cargo bench --bench checker_benchmark\n# CARGO_PROFILE_RELEASE_DEBUG=true cargo run --release --example dhat_heap --features dhat-heap"
    );
    save(&out_dir, "REPRODUCE.sh", &script);

    if fixture.owned && args.cleanup {
        let _ = fs::remove_dir_all(&fixture.dir);
    }

    println!("{report}");
    eprintln!("\nResults written to {}", out_dir.display());

    let tsox_failed = tsox.series.failed();
    if args.strict && (!invalid.is_empty() || vs_tsc.false_positive_total() > 0 || tsox_failed) {
        return Ok(1);
    }
    Ok(0)
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("--__measure") {
        return ExitCode::from(sys::launcher_main(&argv[1..]) as u8);
    }
    match parse_args(&argv) {
        Ok(None) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Some(args)) => match run(&args) {
            Ok(code) => ExitCode::from(code),
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(2)
            }
        },
        Err(error) => {
            eprintln!("error: {error}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(file: &str, line: u32, code: &str) -> Diag {
        Diag {
            file: file.to_owned(),
            line,
            col: 1,
            code: code.to_owned(),
            message: String::new(),
        }
    }

    #[test]
    fn tsc_lines_parse_with_and_without_a_location() {
        let parsed =
            parse_ts_line("src/a.ts(12,5): error TS2322: Type 'string' is not assignable.");
        assert_eq!(
            parsed,
            Some((
                "src/a.ts".to_owned(),
                12,
                5,
                "TS2322".to_owned(),
                "Type 'string' is not assignable.".to_owned()
            ))
        );
        let global = parse_ts_line("error TS2688: Cannot find type definition file for 'x'.");
        assert_eq!(
            global.map(|g| (g.0, g.1, g.3)),
            Some((String::new(), 0, "TS2688".to_owned()))
        );
        assert_eq!(parse_ts_line("Found 1 error."), None);
    }

    #[test]
    fn a_path_with_parentheses_still_parses() {
        let parsed = parse_ts_line("src/a(1).ts(3,4): error TS1005: ';' expected.").unwrap();
        assert_eq!(
            (parsed.0.as_str(), parsed.1, parsed.2),
            ("src/a(1).ts", 3, 4)
        );
    }

    #[test]
    fn continuation_lines_join_the_error_above() {
        let text = "a.ts(1,1): error TS2322: Top.\n  Type 'x' is not assignable to 'y'.\nb.ts(2,2): error TS2345: Next.\n";
        let diags = parse_ts_output(text, Path::new("/p"));
        assert_eq!(diags.len(), 2);
        assert!(diags[0].message.contains("not assignable to 'y'"));
        assert_eq!(diags[1].line, 2);
    }

    #[test]
    fn tsox_lines_split_errors_warnings_and_summaries() {
        let text = "/p/src/a.ts:3:7: error: TS2322 Type mismatch.\n  detail\n/p/src/b.ts:1:1: warning: TSR9014 Not checked.\n\nChecked 31 file(s): 105 error(s), 404 warning(s).\nModule graph: 92 file(s), 2 cycle(s), 0 unresolved import(s).\n/p/src/c.ts: exports partial: no portable form\n";
        let parsed = parse_tsox_output(text, Path::new("/p"));
        assert_eq!(parsed.errors.len(), 1);
        assert_eq!(parsed.errors[0].file, "src/a.ts");
        assert!(parsed.errors[0].message.contains("detail"));
        assert_eq!(parsed.warnings.len(), 1);
        assert_eq!(parsed.checked, Some(31));
        assert_eq!(parsed.reported_errors, Some(105));
        assert_eq!(parsed.graph_files, Some(92));
        assert_eq!(parsed.cycles, Some(2));
        assert_eq!(parsed.unresolved_imports, Some(0));
        assert_eq!(
            parsed.partial,
            vec![("src/c.ts".to_owned(), "no portable form".to_owned())]
        );
    }

    #[test]
    fn compare_counts_matches_false_positives_and_gaps() {
        let reference = vec![
            diag("a.ts", 1, "TS1"),
            diag("a.ts", 2, "TS2"),
            diag("b.ts", 5, "TS3"),
        ];
        let candidate = vec![
            diag("a.ts", 1, "TS1"),
            diag("a.ts", 2, "TS9"),
            diag("c.ts", 7, "TS4"),
        ];
        let c = compare(&reference, &candidate);
        assert_eq!(c.matched, 1);
        assert_eq!(c.false_positive_total(), 2);
        assert_eq!(c.missed_total(), 2);
        assert_eq!(c.lines_both, 2);
        assert_eq!(c.lines_code_differs, 1);
        assert_eq!(c.lines_reference_only, 1);
        assert_eq!(c.lines_candidate_only, 1);
        assert_eq!(c.precision(), Some(1.0 / 3.0));
        assert_eq!(c.recall(), Some(1.0 / 3.0));
    }

    #[test]
    fn precision_is_not_defined_without_candidate_errors() {
        let c = compare(&[diag("a.ts", 1, "TS1")], &[]);
        assert_eq!(c.precision(), None);
        assert_eq!(c.recall(), Some(0.0));
        let clean = compare(&[], &[]);
        assert_eq!((clean.precision(), clean.recall()), (None, None));
    }

    #[test]
    fn a_repeated_error_counts_each_time() {
        let reference = vec![diag("a.ts", 1, "TS1")];
        let candidate = vec![diag("a.ts", 1, "TS1"), diag("a.ts", 1, "TS1")];
        let c = compare(&reference, &candidate);
        assert_eq!(
            (c.matched, c.false_positive_total(), c.missed_total()),
            (1, 1, 0)
        );
    }

    #[test]
    fn files_are_classified() {
        let a = diag("a.ts", 1, "TS1");
        let a_other_code = diag("a.ts", 1, "TS2");
        let b = diag("a.ts", 9, "TS1");
        assert_eq!(classify(&[], &[]), FileClass::Agree);
        assert_eq!(classify(&[&a], &[&a]), FileClass::Agree);
        assert_eq!(classify(&[&a], &[&a_other_code]), FileClass::AgreeLines);
        assert_eq!(classify(&[&a], &[]), FileClass::Gap);
        assert_eq!(classify(&[], &[&a]), FileClass::FalsePositive);
        assert_eq!(classify(&[&a], &[&b]), FileClass::Mixed);
    }

    #[test]
    fn statistics_are_right_on_a_known_sample() {
        let s = stats(&[10.0, 20.0, 30.0, 40.0, 50.0]).unwrap();
        assert_eq!(
            (s.n, s.min, s.max, s.median, s.mean),
            (5, 10.0, 50.0, 30.0, 30.0)
        );
        assert_eq!((s.q1, s.q3), (20.0, 40.0));
        assert!((s.stdev - 15.811388).abs() < 1e-5);
        assert!((s.cv - 52.7046).abs() < 1e-3);
        assert_eq!(stats(&[]).map(|s| s.n), None);
        let one = stats(&[7.0]).unwrap();
        assert_eq!((one.median, one.stdev, one.cv), (7.0, 0.0, 0.0));
    }

    #[test]
    fn dates_convert_from_unix_time() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(1_700_000_000), "20231114T221320Z");
        assert_eq!(utc_pretty(951_782_400), "2000-02-29 00:00:00 UTC");
    }

    #[test]
    fn paths_are_made_relative_and_dependencies_are_not_project_files() {
        let root = Path::new("/p");
        assert_eq!(rel_path(root, "/p/src/a.ts"), "src/a.ts");
        assert_eq!(rel_path(root, "./src/a.ts"), "src/a.ts");
        assert!(is_project_file("src/a.ts"));
        assert!(!is_project_file("node_modules/x/index.d.ts"));
        assert!(!is_project_file("../other/a.ts"));
        assert!(!is_project_file(""));
    }

    #[test]
    fn json_is_escaped_and_nan_becomes_null() {
        let value = obj(vec![
            ("a", jstr("quote \" and \n newline")),
            ("b", jnum(f64::NAN)),
            ("c", jnum(3.0)),
            ("d", jnum(2.5)),
            ("e", J::Arr(vec![])),
        ]);
        let mut out = String::new();
        value.render(0, &mut out);
        assert!(out.contains("\"quote \\\" and \\n newline\""));
        assert!(out.contains("\"b\": null"));
        assert!(out.contains("\"c\": 3,") && out.contains("\"d\": 2.500"));
        assert!(out.contains("\"e\": []"));
    }

    #[test]
    fn extended_diagnostics_lines_are_read_as_key_values() {
        let map = parse_extended(
            "Files:                         312\nMemory used:               224200K\nCheck time:                  2.72s\nnot a metric: text\n",
        );
        assert_eq!(map.get("Files").map(String::as_str), Some("312"));
        assert_eq!(map.get("Check time").map(String::as_str), Some("2.72s"));
        assert_eq!(map.get("not a metric"), None);
    }

    #[test]
    fn arguments_parse_in_both_spellings() {
        let argv: Vec<String> = ["--runs", "3", "--warmup=0", "proj", "--strict"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let args = parse_args(&argv).unwrap().unwrap();
        assert_eq!((args.runs, args.warmup, args.strict), (3, 0, true));
        assert_eq!(args.project, Some(PathBuf::from("proj")));
        assert!(parse_args(&["--runs".to_owned()]).is_err());
        assert!(parse_args(&["--nope".to_owned()]).is_err());
        assert!(parse_args(&["--runs=0".to_owned()]).is_err());
        assert!(parse_args(&["--help".to_owned()]).unwrap().is_none());
    }

    #[test]
    fn the_result_file_of_the_measuring_process_is_parsed() {
        let text = "wall_ms=12.500\nuser_ms=3.000\nsys_ms=1.000\nmax_rss_mb=26.700\nfloor_mb=2.100\nexit_code=3\ntimed_out=0\n";
        let m = parse_measured(text).unwrap();
        assert_eq!((m.wall_ms, m.user_ms, m.sys_ms), (12.5, 3.0, 1.0));
        assert_eq!((m.max_rss_mb, m.floor_mb), (26.7, 2.1));
        assert_eq!((m.exit_code, m.signal, m.timed_out), (Some(3), None, false));
        let killed = parse_measured("wall_ms=1\nsignal=9\ntimed_out=1\n").unwrap();
        assert_eq!(
            (killed.exit_code, killed.signal, killed.timed_out),
            (None, Some(9), true)
        );
        assert_eq!(
            parse_measured("error=cannot start `x`: not found\n").unwrap_err(),
            "cannot start `x`: not found"
        );
        assert!(parse_measured("").is_err());
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    fn launch(name: &str, timeout_ms: u64, command: &[&str]) -> Result<Measured, String> {
        let out = env::temp_dir().join(format!("harness-test-{}-{name}.txt", std::process::id()));
        let mut args = vec![
            out.to_string_lossy().into_owned(),
            timeout_ms.to_string(),
            "--".to_owned(),
        ];
        args.extend(command.iter().map(|s| (*s).to_owned()));
        assert_eq!(sys::launcher_main(&args), 0);
        let text = fs::read_to_string(&out).unwrap();
        let _ = fs::remove_file(&out);
        parse_measured(&text)
    }

    #[cfg(all(unix, target_pointer_width = "64"))]
    #[test]
    fn the_launcher_measures_a_child_and_a_timeout_kills_it() {
        let ok = launch("exit", 10_000, &["sh", "-c", "exit 3"]).unwrap();
        assert_eq!(
            (ok.exit_code, ok.signal, ok.timed_out),
            (Some(3), None, false)
        );
        assert!(ok.wall_ms > 0.0 && ok.max_rss_mb > 0.0 && ok.floor_mb > 0.0);

        let slow = launch("slow", 300, &["sleep", "30"]).unwrap();
        assert!(slow.timed_out);
        assert_eq!(slow.signal, Some(9));
        assert!(slow.wall_ms < 5_000.0);

        let missing = launch("missing", 1_000, &["/no/such/program"]);
        assert!(missing.unwrap_err().contains("cannot start"));
    }
}
