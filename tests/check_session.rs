use tsox::{FileId, ProjectFiles, TypeChecker};

// The session API, the metrics it reports and the file identity types. A session is
// the one place state outlives a check, so these tests are about what must NOT
// survive: types, names and cached answers from the previous file.

#[test]
fn a_session_reports_the_same_diagnostics_as_a_one_shot_check() {
    let source = "const n: number = 'x'; function f(a: string): number { return a; }";
    let one_shot = TypeChecker::new().check_source(source, "m.ts").unwrap();

    let mut session = TypeChecker::new().session(FileId::ROOT);
    let first = session.check_source(source, "m.ts").unwrap();
    let second = session.check_source(source, "m.ts").unwrap();

    let key = |r: &tsox::CheckResult| {
        r.diagnostics
            .iter()
            .map(|d| (d.code, d.start, d.end))
            .collect::<Vec<_>>()
    };
    assert!(!one_shot.diagnostics.is_empty());
    assert_eq!(key(&one_shot), key(&first));
    assert_eq!(key(&one_shot), key(&second));
}

#[test]
fn names_from_the_previous_file_do_not_leak_into_the_next() {
    let mut session = TypeChecker::new().session(FileId::new(3));
    let first = session
        .check_source(
            "interface Only { a: number } const x: Only = { a: 1 };",
            "a.ts",
        )
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{first:?}");

    // `Only` was declared in the previous check; here it must not exist.
    let second = session
        .check_source("const y: Only = { a: 1 };", "b.ts")
        .unwrap();
    assert!(
        !second.diagnostics.is_empty(),
        "an unknown type name must still be reported after a reset"
    );
}

#[test]
fn the_arena_is_reused_and_returns_to_the_same_size() {
    let source = "type Box<T> = { value: T }; const b: Box<number> = { value: 1 };";
    let mut session = TypeChecker::new().session(FileId::ROOT);

    session.check_source(source, "m.ts").unwrap();
    let first = session.last_metrics().unwrap().arena;
    session.check_source(source, "m.ts").unwrap();
    let second = session.last_metrics().unwrap().arena;

    assert_eq!(
        first.type_count, second.type_count,
        "same input, same graph size"
    );
    assert!(
        second.capacity >= first.capacity,
        "capacity is kept, never shrunk"
    );
}

#[test]
fn repeated_generic_instantiations_are_answered_from_the_memo() {
    let source = "type Box<T> = { value: T };\n\
                  const a: Box<number> = { value: 1 };\n\
                  const b: Box<number> = { value: 2 };\n\
                  const c: Box<number> = { value: 3 };";
    let mut session = TypeChecker::new().session(FileId::ROOT);
    let result = session.check_source(source, "m.ts").unwrap();
    assert!(result.diagnostics.is_empty(), "{result:?}");

    let namespace = session.last_metrics().unwrap().namespace;
    assert!(namespace.instantiation_hits >= 2, "{namespace:?}");
    assert_eq!(
        namespace.instantiations, 1,
        "one distinct instantiation: {namespace:?}"
    );
}

#[test]
fn metrics_start_empty_before_any_check() {
    let session = TypeChecker::new().session(FileId::new(9));
    assert!(session.last_metrics().is_none());
    assert_eq!(session.file_id(), FileId::new(9));
}

#[test]
fn project_files_hand_out_one_stable_id_per_path() {
    let mut files = ProjectFiles::new();
    let a = files.intern("src/a.ts");
    let b = files.intern("src/b.ts");
    assert_ne!(a, b);
    assert_eq!(files.intern("src/a.ts"), a);
    assert_eq!(files.len(), 2);
    assert_eq!(a.index(), 0);
}
