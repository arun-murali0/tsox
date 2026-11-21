use tsox::{FileId, Severity, UnitContext, Worker};

// TSR9015: a control-flow rule taken in a simpler form than tsc says so, as a warning.

fn approximated(source: &str) -> usize {
    let ctx = UnitContext::new(FileId::ROOT, "a.ts", source);
    Worker::new()
        .check_unit(&ctx)
        .unwrap()
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning && d.code.as_str() == "TSR9015")
        .count()
}

#[test]
fn a_body_that_runs_on_into_the_next_case_is_reported() {
    assert_eq!(
        approximated("declare var x: number;\nswitch (x) { case 1: x++; case 2: break; }"),
        1
    );
}

#[test]
fn bodies_that_break_return_or_are_empty_are_not() {
    assert_eq!(
        approximated(
            "declare var x: number;\nfunction f() { switch (x) { case 1: case 2: x++; break; case 3: return; default: x--; } }"
        ),
        0
    );
}

#[test]
fn a_catch_entered_without_a_narrowed_name_written_in_the_try_is_reported() {
    let source = "declare var x: string | number;\nif (typeof x === \"string\") {\n  try { x = 1; } catch (e) { x; }\n}";
    assert_eq!(approximated(source), 1);
    assert_eq!(
        approximated("declare var y: string | number;\ntry { y = 1; } catch (e) { y; }"),
        0
    );
}
