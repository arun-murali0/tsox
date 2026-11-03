//! The harness measures a program through a small launcher process so that the harness's own
//! size is not counted as the program's. Measured straight from a big parent, the kernel
//! reports the parent's peak memory as the child's, so this test makes the parent big and checks
//! that a trivial program still reads small.
#![cfg(all(unix, target_pointer_width = "64"))]

use std::fs;
use std::process::Command;

#[test]
fn a_big_parent_does_not_inflate_what_the_measured_program_reads() {
    let mut ballast = vec![0u8; 120 * 1024 * 1024];
    for i in (0..ballast.len()).step_by(4096) {
        ballast[i] = 1;
    }
    std::hint::black_box(&ballast);

    let out =
        std::env::temp_dir().join(format!("harness-launcher-test-{}.txt", std::process::id()));
    let status = Command::new(env!("CARGO_BIN_EXE_harness"))
        .arg("--__measure")
        .arg(&out)
        .arg("10000")
        .arg("--")
        .arg("true")
        .status()
        .expect("the harness binary starts");
    assert!(status.success());
    let text = fs::read_to_string(&out).expect("the launcher wrote its result");
    let _ = fs::remove_file(&out);
    let value = |key: &str| -> f64 {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("no {key} in {text}"))
    };
    // `true` and the launcher need a few MB; 120 MB leaking in from the parent would show here.
    assert!(
        value("max_rss_mb") < 60.0,
        "program read {} MB\n{text}",
        value("max_rss_mb")
    );
    assert!(
        value("floor_mb") < 60.0,
        "launcher floor {} MB\n{text}",
        value("floor_mb")
    );
    drop(ballast);
}
