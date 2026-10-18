#![cfg(feature = "module-resolution")]

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Problem: an input nested deeply enough overflows the stack, which is an abort and takes
// the whole run down (E0). The fuzz run that found this generated each shape at 10,000 and
// 100,000 levels and ran the real binary on it.
// Picked: the same way corpus_aborts works, one generated file in its own project, the real
// binary, and a pass is anything but a signal, a panic exit or a timeout. Where the
// checker's own recursion is what runs out (a member chain, a long binary expression, a
// chain of aliases) the file must also say so with TSR9102, and say it once.
// Cost: the shapes that overflow inside oxc (its parser on nested brackets, ternaries and
// arrows; its semantic builder on very long chains in a debug build) are only tested at a
// depth the 64 MB stack covers. oxc has no limit of its own, so a far deeper one still
// aborts and is not asserted here.
// Now: 10,000 nested brackets, which used to abort on the main thread, run to the end.

const TIMEOUT: Duration = Duration::from_secs(120);

struct Outcome {
    code: Option<i32>,
    output: String,
}

fn run(name: &str, source: &str) -> Outcome {
    let dir: PathBuf = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("depth-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.ts"), source).unwrap();
    fs::write(dir.join("tsconfig.json"), "{}").unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_ts-rust"))
        .arg("--project")
        .arg(dir.join("tsconfig.json"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the binary starts");
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });

    let started = Instant::now();
    let code = loop {
        match child.try_wait().expect("the child can be polled") {
            Some(status) => break status.code(),
            None if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("{name}: no result after {} seconds", TIMEOUT.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let output = reader.join().unwrap();
    let _ = fs::remove_dir_all(&dir);
    Outcome { code, output }
}

// Fails on a signal (a stack overflow aborts with SIGSEGV or SIGABRT) or a panic exit.
fn assert_ran_to_an_end(name: &str, outcome: &Outcome) {
    match outcome.code {
        None => panic!("{name}: killed by a signal (abort or stack overflow)"),
        Some(code) if code >= 100 => panic!("{name}: exit code {code}"),
        Some(_) => {}
    }
}

fn assert_cut_once(name: &str, outcome: &Outcome) {
    assert_ran_to_an_end(name, outcome);
    let cuts = outcome.output.matches("TSR9102").count();
    assert_eq!(
        cuts, 1,
        "{name}: expected exactly one nesting diagnostic, got {cuts}:\n{}",
        outcome.output
    );
}

// The three flat chains below are walked by oxc's semantic builder before the checker runs,
// and in a debug build its frames are big enough (about 1.3 KB a level) that it overflows
// the 64 MB stack at around 50,000 levels, before the guard has anything to say. So a debug
// build is only held to the depth it can survive, and the cut is asserted in a release
// build, where the checker's own recursion is what runs out first.
#[cfg(debug_assertions)]
const FLAT_CHAIN: usize = 20_000;
#[cfg(not(debug_assertions))]
const FLAT_CHAIN: usize = 100_000;

fn assert_flat_chain(name: &str, source: &str) {
    let outcome = run(name, source);
    if cfg!(debug_assertions) {
        assert_ran_to_an_end(name, &outcome);
    } else {
        assert_cut_once(name, &outcome);
    }
}

#[test]
fn a_very_long_member_chain_is_cut_off_and_reported_once() {
    let source = format!(
        "declare const a: any;\nlet x = a{};\n",
        ".b".repeat(FLAT_CHAIN)
    );
    assert_flat_chain("member", &source);
}

#[test]
fn a_very_long_binary_expression_is_cut_off_and_reported_once() {
    let source = format!("let x = 1{};\n", " + 1".repeat(FLAT_CHAIN));
    assert_flat_chain("binary", &source);
}

#[test]
fn a_very_long_optional_chain_is_cut_off_and_reported_once() {
    let source = format!(
        "declare const a: any;\nlet x = a{};\n",
        "?.b".repeat(FLAT_CHAIN)
    );
    assert_flat_chain("optional", &source);
}

// A type written as `number[][][]...` is resolved one level per call, and it is then
// compared with itself by assignment, which walks it again. The array suffix is parsed
// without recursion, so this reaches the checker's own walk at a depth the parser allows.
#[cfg(debug_assertions)]
const DEEP_TYPE: usize = 20_000;
#[cfg(not(debug_assertions))]
const DEEP_TYPE: usize = 1_000_000;

#[test]
fn a_very_deep_array_type_is_cut_off_and_reported_once() {
    let source = format!(
        "type T = number{};\ndeclare const a: T;\nlet b: T = a;\n",
        "[]".repeat(DEEP_TYPE)
    );
    assert_flat_chain("array-type", &source);
}

#[test]
fn a_very_long_alias_chain_is_cut_off_and_reported_once() {
    let mut source = String::new();
    for index in 0..200_000 {
        source.push_str(&format!("type A{index} = A{};\n", index + 1));
    }
    source.push_str("type A200000 = number;\nlet x: A0;\n");
    assert_cut_once("alias", &run("alias", &source));
}

#[test]
fn ordinary_deep_nesting_is_not_cut_off() {
    // Real code does not go past a few hundred levels; this is well inside every limit.
    let source = format!(
        "declare const a: any;\nlet x = a{};\nlet y = 1{};\n",
        ".b".repeat(500),
        " + 1".repeat(500)
    );
    let outcome = run("ordinary", &source);
    assert_ran_to_an_end("ordinary", &outcome);
    assert!(
        !outcome.output.contains("TSR9102"),
        "a file nested 500 deep was cut off:\n{}",
        outcome.output
    );
}

#[test]
fn ten_thousand_nested_brackets_run_to_an_end() {
    // These overflowed the 8 MB main thread that parsed the file during discovery.
    let parens = format!("let x = {}1{};\n", "(".repeat(10_000), ")".repeat(10_000));
    assert_ran_to_an_end("parens", &run("parens", &parens));

    let arrays = format!("let x = {}{};\n", "[".repeat(10_000), "]".repeat(10_000));
    assert_ran_to_an_end("arrays", &run("arrays", &arrays));

    let generics = format!(
        "type T = {}number{};\n",
        "Array<".repeat(10_000),
        ">".repeat(10_000)
    );
    assert_ran_to_an_end("generics", &run("generics", &generics));
}
