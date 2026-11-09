// The backward flow resolver (F2), run beside the forward narrowing overlay.
//
// Problem: the overlay threads narrowing forward through the statements and has no second opinion;
// the flow graph can answer "what is the type of this read" by walking back from the read.
// Picked: the resolver runs in shadow mode (off by default, `TSOX_FLOW_SHADOW`), reuses the
// overlay's own narrowing rules for one test or one assignment, and compares its answer with the
// overlay's at every read, reporting each difference as a warning.
// Now: these tests pin that it is off by default, that it agrees with the overlay on the
// constructs both understand, and that a difference it is known to find is still found, so fixing
// the cause means updating that test on purpose.

use tsox::{CheckOptions, DiagnosticCode, FileId, UnitContext, Worker};
use tsox_checker::bridge::flow_shadow::{
    enable_for_current_thread, report_every_read_for_current_thread,
};

fn run(source: &str, shadow: bool) -> (usize, usize) {
    enable_for_current_thread(shadow);
    report_every_read_for_current_thread(shadow);
    let options = CheckOptions {
        strict: true,
        strict_null_checks: true,
        ..CheckOptions::default()
    };
    let mut worker = Worker::new();
    let ctx = UnitContext::new(FileId::ROOT, "t.ts", source).with_options(options);
    let outcome = worker.check_unit(&ctx).expect("the unit checks");
    let count = |code| {
        outcome
            .diagnostics
            .iter()
            .filter(|d| d.code == code)
            .count()
    };
    (
        count(DiagnosticCode::FlowShadowRead),
        count(DiagnosticCode::FlowShadowDisagreement),
    )
}

#[test]
fn it_is_off_unless_asked_for() {
    let (agreed, disagreed) = run(
        "function f(u: string | number) { if (typeof u === \"string\") { u; } }",
        false,
    );
    assert_eq!((agreed, disagreed), (0, 0));
}

#[test]
fn it_agrees_with_the_overlay_on_tests_assignments_loops_and_switches() {
    let source = r#"
function tests(u: string | number | null) {
  if (typeof u === "string") { u; } else { u; }
  if (u === null) { u; } else { u; }
  if (!u) { u; } else { u; }
  u;
}
function assigned(u: string | number) {
  u = "a";
  u;
  if (typeof u === "number") { u; }
}
function loops(u: string | number) {
  while (typeof u === "string") { u = 1; }
  u;
  for (let i = 0; i < 3; i++) {
    if (typeof u === "string") { u = 1; }
    u;
  }
  u;
}
function switches(k: "a" | "b" | "c") {
  switch (k) {
    case "a": k; break;
    case "b":
    case "c": k; break;
  }
  k;
}
export {};
"#;
    let (agreed, disagreed) = run(source, true);
    assert_eq!(disagreed, 0, "the two differ on code they both understand");
    assert!(agreed > 20, "the walk answered only {agreed} reads");
}

// Known: the flow graph enters a `catch` from the end of its `try` only, where tsc (and the
// overlay) enter it from the start and from after every assignment, so after `u = 1` the walk
// says `number` in the `catch` and the overlay says the declared type. When F1 builds the
// exception edges from every point this test will fail, which is the cue to turn it into an
// agreement case.
#[test]
fn it_finds_the_known_catch_entry_difference() {
    let (_, disagreed) = run(
        "function f(u: string | number | undefined) { try { u = 1; } catch { u; } }",
        true,
    );
    assert!(disagreed > 0);
}

// Known: the overlay does not narrow a clause entered by falling through from the one above it
// (in `case \"number\"` after `case \"string\"` the variable is still `string` there), so it says
// `number` where the walk, like tsc, says `string | number`.
#[test]
fn it_finds_the_overlay_missing_a_fallthrough() {
    let (_, disagreed) = run(
        "function f(u: string | number) { switch (typeof u) { case \"string\": u; case \"number\": u; } }",
        true,
    );
    assert!(disagreed > 0);
}
