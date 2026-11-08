//! `RecordingImports` must forward and record every method of `ImportSource` and
//! `DeclProvider`. A method added to a trait and not to the recorder would fall back to the
//! trait's default, go unrecorded, and let a cached output be used when the answer it depends
//! on has changed. This reads the trait definitions and the recorder as text and fails when
//! the two drift.
#![cfg(feature = "snapshot")]

use std::fs;
use std::path::Path;

fn trait_methods(source: &str, header: &str) -> Vec<String> {
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("no `{header}`"));
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the trait ends");
    body[..end]
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("fn "))
        .map(|rest| rest.split(['(', '<']).next().unwrap().trim().to_string())
        .collect()
}

#[test]
fn the_recorder_covers_every_method_of_the_import_traits() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let portable = fs::read_to_string(root.join("crates/tsox-types/src/portable.rs")).unwrap();
    let recorder = fs::read_to_string(root.join("crates/tsox-checker/src/probe_log.rs")).unwrap();

    let mut methods = trait_methods(&portable, "pub trait DeclProvider");
    methods.extend(trait_methods(
        &portable,
        "pub trait ImportSource: DeclProvider",
    ));
    assert!(methods.len() >= 12, "found {methods:?}");

    for method in &methods {
        assert!(
            recorder.contains(&format!("fn {method}(")),
            "RecordingImports does not forward `{method}`; add it, and a ProbeCall for it"
        );
    }
}

// -- a stored output is used only when the project answers as it did -----------------------

use std::collections::HashMap;
use std::sync::Arc;

use tsox::{
    DeclKey, DeclProvider, FileId, ImportSource, ModuleKey, PortableDecl, UnitContext, UnitOutput,
    Worker,
};
use tsox_checker::probe_log::{FingerprintMemo, RecordingImports, declarations_read};
use tsox_checker::unit_snapshot::{CacheEntry, UnitSnapshot, Variant};

#[derive(Default)]
struct Project {
    modules: HashMap<String, Vec<(String, DeclKey)>>,
    globals: HashMap<String, Vec<DeclKey>>,
    decls: HashMap<DeclKey, Arc<PortableDecl>>,
}

impl Project {
    fn with_module(mut self, specifier: &str, source: &str) -> Self {
        let out = unit(specifier, source, None);
        self.modules
            .insert(specifier.to_string(), out.export_names.clone());
        for decl in &out.exports {
            self.decls.insert(decl.key, Arc::new(decl.clone()));
        }
        self
    }

    // A script's top-level declarations, as the project's globals.
    fn with_global_script(mut self, source: &str) -> Self {
        let out = unit("globals.ts", source, None);
        for block in &out.globals {
            for (name, key) in &block.export_names {
                self.globals.entry(name.clone()).or_default().push(*key);
            }
            for decl in &block.exports {
                self.decls.insert(decl.key, Arc::new(decl.clone()));
            }
        }
        self
    }
}

impl DeclProvider for Project {
    fn decl(&self, key: DeclKey) -> Option<Arc<PortableDecl>> {
        self.decls.get(&key).cloned()
    }
}

impl ImportSource for Project {
    fn export_keys(&self, specifier: &str, name: &str) -> Vec<DeclKey> {
        self.modules
            .get(specifier)
            .into_iter()
            .flatten()
            .filter(|(n, _)| n == name)
            .map(|(_, k)| *k)
            .collect()
    }

    fn global_decls(&self, name: &str) -> Vec<DeclKey> {
        self.globals.get(name).cloned().unwrap_or_default()
    }
}

fn unit(file: &str, source: &str, imports: Option<&dyn ImportSource>) -> UnitOutput {
    let mut ctx =
        UnitContext::new(FileId::ROOT, file, source).with_module(ModuleKey::project(file));
    if let Some(imports) = imports {
        ctx = ctx.with_imports(imports);
    }
    Worker::new().check_unit(&ctx).unwrap()
}

// The unit run through a recorder, as the driver runs a cacheable file.
fn recorded(source: &str, project: &Project) -> Variant {
    let recorder = RecordingImports::new(project);
    let output = unit("dep.d.ts", source, Some(&recorder));
    Variant {
        probes: recorder.into_records(),
        snapshot: UnitSnapshot::capture(&output),
    }
}

fn entry_of(variants: impl IntoIterator<Item = Variant>) -> CacheEntry {
    let mut entry = CacheEntry::new();
    for variant in variants {
        entry.push(variant);
    }
    entry
}

#[test]
fn an_output_made_without_a_global_is_not_given_to_a_project_that_declares_it() {
    let dep = "export declare function make(): Foo;\n";
    let without = Project::default();
    let with = Project::default().with_global_script("interface Foo { x: string }\n");

    let made_without = recorded(dep, &without);
    let made_with = recorded(dep, &with);
    // The test is only worth anything if the global changes the unit's output.
    assert_ne!(
        made_without.snapshot, made_with.snapshot,
        "declaring Foo must change what `make` exports"
    );

    let entry = entry_of([made_without.clone()]);
    let (hit, _) = entry
        .lookup(&without, None)
        .expect("the project it was made for");
    assert_eq!(
        hit.exports,
        made_without.snapshot.clone().into_output().exports
    );
    assert!(
        entry.lookup(&with, None).is_none(),
        "a name nobody declared when the output was made is declared now"
    );

    // Both outputs live in one entry, and each project gets its own.
    let entry = entry_of([made_without.clone(), made_with.clone()]);
    assert_eq!(entry.len(), 2);
    assert_eq!(
        entry.lookup(&without, None).unwrap().0.exports,
        made_without.snapshot.clone().into_output().exports
    );
    assert_eq!(
        entry.lookup(&with, None).unwrap().0.exports,
        made_with.snapshot.clone().into_output().exports
    );
}

#[test]
fn an_output_made_against_one_export_is_not_given_to_a_project_with_another() {
    let dep = "import { User } from './a';\nexport declare const u: User;\n";
    let first = Project::default().with_module("./a", "export interface User { name: string }");
    let second = Project::default().with_module("./a", "export interface User { name: number }");

    let made = recorded(dep, &first);
    let entry = entry_of([made.clone()]);
    assert!(entry.lookup(&first, None).is_some());
    assert!(
        entry.lookup(&second, None).is_none(),
        "the imported declaration is not the one the output was made against"
    );
}

#[test]
fn a_hit_reports_the_reads_a_fresh_check_reports() {
    let dep = "import { User } from './a';\nexport declare const u: User;\n";
    let project = Project::default().with_module("./a", "export interface User { name: string }");
    let made = recorded(dep, &project);
    let reads = declarations_read(&made.probes);
    assert!(
        !reads.is_empty(),
        "the import is read through a declaration"
    );
    let (_, from_hit) = entry_of([made]).lookup(&project, None).unwrap();
    assert_eq!(from_hit, reads);
}

#[test]
fn an_entry_keeps_a_bounded_number_of_outputs_and_replaces_one_made_under_the_same_answers() {
    let project = Project::default();
    let mut entry = CacheEntry::new();
    entry.push(recorded("export declare const a: number;\n", &project));
    entry.push(recorded("export declare const a: string;\n", &project));
    // The same questions and answers: the newer output replaces the older.
    assert_eq!(entry.len(), 1);
    let (hit, _) = entry.lookup(&project, None).unwrap();
    assert!(
        format!("{:?}", hit.exports).contains("String"),
        "{:?}",
        hit.exports
    );
}

// -- hits that move the output out, and the memo of fingerprints ---------------------------

#[test]
fn take_hit_gives_what_lookup_gives_and_removes_only_that_variant() {
    let dep = "export declare function make(): Foo;\n";
    let without = Project::default();
    let with = Project::default().with_global_script("interface Foo { x: string }\n");
    let mut entry = entry_of([recorded(dep, &without), recorded(dep, &with)]);

    let (looked_up, reads) = entry.lookup(&with, None).unwrap();
    let (taken, taken_reads) = entry.take_hit(&with, None).unwrap();
    assert_eq!(taken.exports, looked_up.exports);
    assert_eq!(taken.diagnostics, looked_up.diagnostics);
    assert_eq!(taken_reads, reads);

    // The variant made for the other project is still there; the one taken is gone.
    assert_eq!(entry.len(), 1);
    assert!(entry.lookup(&without, None).is_some());
    assert!(entry.take_hit(&with, None).is_none());
}

#[test]
fn a_take_hit_miss_leaves_the_entry_whole() {
    let dep = "export declare function make(): Foo;\n";
    let without = Project::default();
    let with = Project::default().with_global_script("interface Foo { x: string }\n");
    let mut entry = entry_of([recorded(dep, &without)]);
    assert!(entry.take_hit(&with, None).is_none());
    assert_eq!(entry.len(), 1, "a miss takes nothing out");
    // So a variant for the project that missed can still be added, and both are kept.
    entry.push(recorded(dep, &with));
    assert_eq!(entry.len(), 2);
}

#[test]
fn memoized_fingerprints_decide_exactly_as_unmemoized_ones_do() {
    let dep = "import { User } from './a';\nexport declare const u: User;\nexport declare function make(): Foo;\n";
    let first = Project::default()
        .with_module("./a", "export interface User { name: string }")
        .with_global_script("interface Foo { x: string }\n");
    let same = Project::default()
        .with_module("./a", "export interface User { name: string }")
        .with_global_script("interface Foo { x: string }\n");
    let changed = Project::default()
        .with_module("./a", "export interface User { name: number }")
        .with_global_script("interface Foo { x: string }\n");

    let memo = FingerprintMemo::new();
    let entry = entry_of([recorded(dep, &first)]);
    for project in [&first, &same, &changed] {
        let plain = entry.lookup(project, None).is_some();
        // Asked twice through the same memo: the first fills it, the second reads it.
        assert_eq!(entry.lookup(project, Some(&memo)).is_some(), plain);
        assert_eq!(entry.lookup(project, Some(&memo)).is_some(), plain);
    }
    assert!(
        !memo.is_empty(),
        "the declarations were hashed through the memo"
    );

    // A recording made through a memo stores the same questions and answers as one made
    // without.
    let recorder = RecordingImports::with_memo(&first, &memo);
    let output = unit("dep.d.ts", dep, Some(&recorder));
    let through_memo = Variant {
        probes: recorder.into_records(),
        snapshot: UnitSnapshot::capture(&output),
    };
    assert_eq!(through_memo.probes, recorded(dep, &first).probes);
}

#[test]
fn the_memo_keeps_a_declaration_alive_so_its_address_cannot_be_reused() {
    let project = Project::default().with_global_script("interface Foo { x: string }\n");
    let key = *project.decls.keys().next().expect("a global declaration");
    let held = project.decl(key).unwrap();
    let before = Arc::strong_count(&held);

    let memo = FingerprintMemo::new();
    let entry = entry_of([recorded("export declare function make(): Foo;\n", &project)]);
    assert!(entry.lookup(&project, Some(&memo)).is_some());
    assert_eq!(memo.len(), 1);
    // The memo took its own reference to what it hashed: the allocation outlives every other
    // holder, which is what makes the address a safe name for the declaration.
    assert!(Arc::strong_count(&held) > before);

    // A different declaration never answers for it, whatever address it has.
    let other = Project::default().with_global_script("interface Foo { x: number }\n");
    assert!(entry.lookup(&other, Some(&memo)).is_none());
    assert!(entry.lookup(&project, Some(&memo)).is_some());
}
