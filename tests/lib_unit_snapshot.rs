//! A checked library file survives the byte form unchanged (stage 1 of the library snapshot):
//! the unit output of every bundled file of the default set round-trips exactly, and a reader
//! refuses bytes it should not trust. Nothing reads a snapshot in a check yet.
#![cfg(feature = "snapshot")]

use tsox::{
    CheckOptions, EffectiveCompilerOptions, FileId, ModuleKey, ScriptTarget, SnapshotError,
    StandardLibrarySet, UnitContext, UnitSnapshot, Worker,
};

fn lib_outputs() -> Vec<(String, tsox::UnitOutput)> {
    lib_outputs_for(false)
}

fn lib_outputs_for(without_dom: bool) -> Vec<(String, tsox::UnitOutput)> {
    let options = EffectiveCompilerOptions {
        target: ScriptTarget::Es2020,
        ..EffectiveCompilerOptions::default()
    };
    let mut set = StandardLibrarySet::for_options(&options).unwrap();
    if without_dom {
        set = set.without_dom_family();
    }
    let mut worker = Worker::new();
    set.files()
        .iter()
        .map(|file| {
            let source = file.read_source().unwrap();
            let name = format!("lib.{}.d.ts", file.name);
            let ctx = UnitContext::new(FileId::ROOT, &name, &source)
                .with_module(ModuleKey::project(&name))
                .with_options(CheckOptions::default());
            let output = worker.check_unit(&ctx).unwrap();
            (name, output)
        })
        .collect()
}

#[test]
fn every_default_library_unit_round_trips_exactly() {
    let started = std::time::Instant::now();
    let outputs = lib_outputs();
    eprintln!(
        "checking the {} library units took {:?}",
        outputs.len(),
        started.elapsed()
    );
    assert!(
        outputs.len() > 40,
        "the default set is {} files",
        outputs.len()
    );
    let mut total_bytes = 0;
    let mut carried = 0;
    let mut diagnostics = 0;
    for (name, output) in &outputs {
        let snapshot = UnitSnapshot::capture(output);
        let bytes = snapshot.encode().unwrap();
        total_bytes += bytes.len();
        let decoded = UnitSnapshot::decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, snapshot, "{name}");
        // Encoding the decoded value gives the same bytes: the form is canonical.
        assert_eq!(decoded.encode().unwrap(), bytes, "{name}");
        let back = decoded.into_output();
        assert_eq!(back.globals, output.globals, "{name}");
        assert_eq!(back.exports, output.exports, "{name}");
        assert_eq!(back.incomplete, output.incomplete, "{name}");
        assert_eq!(back.diagnostics, output.diagnostics, "{name}");
        diagnostics += back.diagnostics.len();
        carried += back.globals.len() + back.exports.len();
    }
    eprintln!(
        "{} units, {total_bytes} bytes, {carried} carried items",
        outputs.len()
    );
    assert!(
        diagnostics > 0,
        "no library unit reported anything, so diagnostics were not exercised"
    );
    assert!(
        carried > 0,
        "the library units carried nothing, so the test proves nothing"
    );
}

#[test]
fn a_reader_refuses_other_formats_and_damaged_bytes() {
    let outputs = lib_outputs();
    let (_, output) = outputs
        .iter()
        .find(|(_, o)| !o.diagnostics.is_empty())
        .expect("a library unit with diagnostics");
    let bytes = UnitSnapshot::capture(output).encode().unwrap();

    let mut other_format = bytes.clone();
    other_format[0] = other_format[0].wrapping_add(1);
    assert!(matches!(
        UnitSnapshot::decode(&other_format),
        Err(SnapshotError::Format { .. })
    ));

    let mut other_encoding = bytes.clone();
    other_encoding[4] = other_encoding[4].wrapping_add(1);
    assert!(matches!(
        UnitSnapshot::decode(&other_encoding),
        Err(SnapshotError::Encoding { .. })
    ));

    assert!(matches!(
        UnitSnapshot::decode(&bytes[..bytes.len() / 2]),
        Err(SnapshotError::Damaged(_))
    ));
    assert!(matches!(
        UnitSnapshot::decode(&[]),
        Err(SnapshotError::Damaged(_))
    ));
}

#[test]
#[ignore = "timing, run with --release --ignored --nocapture"]
fn time_the_es_only_units() {
    for _ in 0..3 {
        let started = std::time::Instant::now();
        let outputs = lib_outputs_for(true);
        eprintln!(
            "ES-only: {} units in {:?}",
            outputs.len(),
            started.elapsed()
        );
    }
}
