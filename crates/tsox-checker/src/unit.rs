use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use crate::arena::TypeArena;
use crate::bridge::{self, Checked, ExportTable, IncompleteReason};
use crate::diagnostic_messages::messages;
use crate::diagnostics::{Diagnostic, Severity};
use crate::error::CheckerError;
use crate::keys::{DeclKey, ModuleKey};
use crate::portable::ImportSource;
use crate::portable::PortableDecl;
use crate::types::FileId;

// A flag a caller raises to stop a unit that is no longer wanted, such as a file edited
// again while its last check is still running. Clones share the one flag, so the caller
// keeps one and the unit gets another. Raising it is advisory: the checker looks at the
// start of a unit and every few statements, not between every operation.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

// What one unit of work reads and never changes. In the code a unit is one file, checked
// in its own arena. The graph driver (check_project) schedules a circular import group as
// one unit of settlement, re-checking its members until their exports stop changing, and
// gives each file what it imports through `imports` (LLD 4.3, HLD).
pub struct UnitContext<'a> {
    pub file_id: FileId,
    pub file_name: &'a str,
    pub source: &'a str,
    pub cancel: Option<CancelToken>,
    // The file's stable identity. A unit given one reports its annotated exports; a unit
    // without one has no way to key a declaration and exports nothing.
    pub module: Option<ModuleKey>,
    // Where the file's imports resolve. A unit without one leaves its import
    // declarations unbound, as a single-file check does.
    pub imports: Option<&'a dyn ImportSource>,
    // The compiler options the checker reads. The default is the checker's fixed behavior.
    pub options: crate::check_options::CheckOptions,
    // Leave out the pass that checks the file's statements and declarations for errors, for
    // a declaration file whose errors nobody reads. What the unit hands back to other files
    // (its exports, ambient modules, globals, the names it uses) is the same either way, and
    // only the errors that pass finds are missing; the ones found while declaring the file
    // and collecting its exports are still there. A file that has a construct which could
    // need the pass (an initializer, for one: `declare const n = 42` gets its type from
    // there) is checked in full whatever this says, so asking is always safe.
    pub skip_bodies: bool,
    // A question about the type at an offset, answered while the unit is checked.
    pub probe: Option<&'a bridge::ProbeRequest>,
}

impl<'a> UnitContext<'a> {
    pub fn new(file_id: FileId, file_name: &'a str, source: &'a str) -> Self {
        Self {
            file_id,
            file_name,
            source,
            cancel: None,
            module: None,
            imports: None,
            options: crate::check_options::CheckOptions::default(),
            skip_bodies: false,
            probe: None,
        }
    }

    pub fn with_module(mut self, module: ModuleKey) -> Self {
        self.module = Some(module);
        self
    }

    pub fn with_imports(mut self, imports: &'a dyn ImportSource) -> Self {
        self.imports = Some(imports);
        self
    }

    /// Asks to skip the error-checking pass (see `skip_bodies`): the errors it would find are
    /// not reported, and everything other files read from the unit is unchanged.
    pub fn without_body_checks(mut self) -> Self {
        self.skip_bodies = true;
        self
    }

    pub fn with_options(mut self, options: crate::check_options::CheckOptions) -> Self {
        self.options = options;
        self
    }

    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn with_probe(mut self, probe: &'a bridge::ProbeRequest) -> Self {
        self.probe = Some(probe);
        self
    }
}

// Everything a unit hands back, and nothing that points into its arena: no TypeId, no
// borrow. The compile-time check below keeps it that way for Send and 'static, and
// LLD 2.1 rule 3 keeps TypeId out by review.
//
// incomplete is set when the diagnostics cannot be taken as the file's full list, which
// today means the checker crashed on it. Units that depend on this one will read it to
// treat its exports as unknown instead of trusting them.
#[derive(Debug)]
pub struct UnitOutput {
    /// What the check learned about the file, as arena-free tables (see `tsox-model`). Today it
    /// holds `module_info`; the other tables are filled as their writers move over.
    pub model: tsox_model::SemanticModel,
    /// The compiler options this unit read. Under any other values of the options it did not
    /// read, the output is the same, so a change to one of those needs no re-check.
    pub options_read: crate::check_options::OptionReads,
    pub diagnostics: Vec<Diagnostic>,
    pub incomplete: bool,
    // Why the unit is partial, sorted and without duplicates: the reasons of its own export
    // list and of the `declare module` blocks and augmentations in it. Empty when the
    // exports are complete.
    pub incomplete_reasons: Vec<IncompleteReason>,
    // The unit's exported declarations as owned, arena-free data (LLD 3.4). Filled when
    // the unit has a module key: annotated interfaces, aliases, functions and constants.
    // Anything exported that the portable form cannot carry yet is left out and sets
    // `incomplete`, so a dependent sees less, never something wrong.
    pub exports: Vec<PortableDecl>,
    // Export name to the key of the declaration under it. A name that is both a type and
    // a value appears once for each; the declaration's `kind` tells them apart.
    pub export_names: Vec<(String, DeclKey)>,
    // True when `export_names` is the module's whole export list. A dependent reports a
    // missing name only against a complete list; against a partial one the name may be
    // an export that was left out.
    pub exports_complete: bool,
    // The modules this unit declares with `declare module "x" { ... }`, which answer the
    // specifier `x` for every file of the project.
    pub ambient: Vec<AmbientExports>,
    /// `export as namespace Name` aliases that expose this module's value exports as a
    /// global namespace in script consumers.
    pub umd_namespaces: Vec<String>,
    /// The names the file uses that nothing in it declares.
    pub unresolved_names: Vec<String>,
    /// Module declarations inside an external module augment an existing module instead
    /// of introducing a standalone ambient module.
    pub augmentations: Vec<AmbientExports>,
    /// What this file declares globally: the top-level declarations of a script file and the
    /// members of each `declare global { }` block. At most one entry, with an empty
    /// specifier; the names are not exports of the file.
    pub globals: Vec<AmbientExports>,
    /// Exported functions with no return annotation whose return expressions reach an imported
    /// function without one. The project joins these across files to find a circle through
    /// imports (TS7023), which no single file can see.
    pub return_reads: Vec<ReturnRead>,
}

/// One exported function whose inferred return type is read from imported functions, directly
/// or through functions of its own file (see `UnitOutput::return_reads`).
#[cfg_attr(feature = "snapshot", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReturnRead {
    pub owner: DeclKey,
    pub name: String,
    pub start: u32,
    pub end: u32,
    /// The imported functions its return expressions reach, sorted and without duplicates.
    pub callees: Vec<DeclKey>,
    /// True when the file already reported this function as part of a circle of its own.
    pub reported_locally: bool,
}

/// The exports of one `declare module "x"` block (see `UnitOutput`).
#[cfg_attr(feature = "snapshot", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmbientExports {
    pub specifier: String,
    pub exports: Vec<PortableDecl>,
    pub export_names: Vec<(String, DeclKey)>,
    pub complete: bool,
    /// Global names (see `UnitOutput::globals`) whose declaration could not be carried over.
    pub unmodelled: Vec<String>,
    /// The type parameters (name, has a default) of the generic ones among `unmodelled`.
    pub unmodelled_generics: Vec<(String, Vec<(String, bool)>)>,
    /// The names declared `const` among the globals, which no file may assign to.
    pub consts: Vec<String>,
    /// Of `unmodelled`, the names dropped as types only and the names dropped with a runtime
    /// side. A name that is only in the first has no value.
    pub unmodelled_types: Vec<String>,
    pub unmodelled_values: Vec<String>,
    /// `true` for a script's globals when only the names were listed (`unmodelled`), not the
    /// declarations. The project builds the declarations in a later run if another file uses
    /// one of the names.
    pub names_only: bool,
    /// `true` for `declare module "x"` inside an external module.
    pub augmentation: bool,
}

const _: fn() = || {
    fn owned_and_sendable<T: Send + 'static>() {}
    owned_and_sendable::<UnitOutput>();
};

#[derive(Debug, Error)]
pub enum UnitError {
    // A cancelled unit has no output and is never cached (LLD 5.5).
    #[error("the unit was cancelled")]
    Cancelled,
    #[error(transparent)]
    Checker(#[from] CheckerError),
}

// One per thread that checks units. It owns the arena and hands it to each unit in
// turn, cleared, so the allocation survives from one unit to the next and nothing a unit
// declared is visible to the one after it.
pub struct Worker {
    arena: TypeArena,
}

impl Worker {
    pub fn new() -> Self {
        Self {
            arena: TypeArena::new(),
        }
    }

    // Runs f against the worker's arena and resets the arena afterwards. R is owned and
    // Send so a result cannot keep a borrow of the arena alive past the reset.
    pub fn run_unit<R>(&mut self, _ctx: &UnitContext<'_>, f: impl FnOnce(&mut TypeArena) -> R) -> R
    where
        R: Send + 'static,
    {
        let out = f(&mut self.arena);
        self.arena.clear();
        out
    }

    pub fn check_unit(&mut self, ctx: &UnitContext<'_>) -> Result<UnitOutput, UnitError> {
        self.contained(ctx, |arena| {
            bridge::check_file(
                ctx.source,
                ctx.file_name,
                ctx.file_id,
                arena,
                bridge::FileEnv {
                    cancel: ctx.cancel.as_ref(),
                    module: ctx.module,
                    imports: ctx.imports,
                    options: ctx.options,
                    skip_bodies: ctx.skip_bodies,
                    probe: ctx.probe,
                },
            )
        })
    }

    // A panic inside the check is caught here and turned into an output with one
    // internal-error diagnostic, marked incomplete, so one bad file does not take the
    // worker down with it. The arena is replaced rather than cleared: a panic can leave
    // it half-built, and a fresh one is the only state known to be sound.
    fn contained(
        &mut self,
        ctx: &UnitContext<'_>,
        check: impl FnOnce(&mut TypeArena) -> Result<Option<Checked>, CheckerError>,
    ) -> Result<UnitOutput, UnitError> {
        let ran = catch_unwind(AssertUnwindSafe(|| self.run_unit(ctx, check)));

        match ran {
            Ok(Ok(Some((diagnostics, _metrics, table)))) => {
                let partial = table.is_incomplete();
                let reasons = all_reasons(&table);
                Ok(UnitOutput {
                    model: model_of(&table),
                    options_read: table.options_read,
                    diagnostics,
                    incomplete: partial,
                    incomplete_reasons: reasons,
                    exports: table.decls,
                    export_names: table.names,
                    exports_complete: table.collected && !partial,
                    ambient: table
                        .ambient
                        .into_iter()
                        .map(|(specifier, ambient)| AmbientExports {
                            specifier,
                            complete: ambient.collected && !ambient.is_incomplete(),
                            exports: ambient.decls,
                            export_names: ambient.names,
                            unmodelled: Vec::new(),
                            unmodelled_generics: Vec::new(),
                            consts: Vec::new(),
                            unmodelled_types: Vec::new(),
                            unmodelled_values: Vec::new(),
                            names_only: false,
                            augmentation: false,
                        })
                        .collect(),
                    umd_namespaces: table.umd_namespaces,
                    unresolved_names: table.unresolved_names,
                    return_reads: table.return_reads,
                    globals: if table.global_names.is_empty() && table.global_unmodelled.is_empty()
                    {
                        Vec::new()
                    } else {
                        vec![AmbientExports {
                            specifier: String::new(),
                            complete: true,
                            unmodelled: table.global_unmodelled,
                            unmodelled_generics: table.global_generics,
                            consts: table.global_consts,
                            unmodelled_types: table.global_unmodelled_types,
                            unmodelled_values: table.global_unmodelled_values,
                            names_only: table.global_names_only,
                            exports: table.global_decls,
                            export_names: table.global_names,
                            augmentation: false,
                        }]
                    },
                    augmentations: table
                        .augmentations
                        .into_iter()
                        .map(|(specifier, augmentation)| AmbientExports {
                            specifier,
                            complete: augmentation.collected && !augmentation.is_incomplete(),
                            exports: augmentation.decls,
                            export_names: augmentation.names,
                            unmodelled: Vec::new(),
                            unmodelled_generics: Vec::new(),
                            consts: Vec::new(),
                            unmodelled_types: Vec::new(),
                            unmodelled_values: Vec::new(),
                            names_only: false,
                            augmentation: true,
                        })
                        .collect(),
                })
            }
            Ok(Ok(None)) => Err(UnitError::Cancelled),
            Ok(Err(error)) => Err(error.into()),
            Err(_panic) => {
                self.arena = TypeArena::new();
                Ok(crashed(ctx))
            }
        }
    }
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}

// The reasons of the unit's own list and of every ambient block and augmentation in it.
fn all_reasons(table: &ExportTable) -> Vec<IncompleteReason> {
    let mut reasons = table.incomplete_reasons.clone();
    for (_, block) in table.ambient.iter().chain(table.augmentations.iter()) {
        reasons.extend(block.incomplete_reasons.iter().copied());
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

// The model of one checked file. Exports and imports are read from the export table, which
// is what the checker already collected; the model sorts them so it does not depend on the
// order they were found in.
fn model_of(table: &ExportTable) -> tsox_model::SemanticModel {
    let mut exports: Vec<(Box<str>, DeclKey)> = table
        .names
        .iter()
        .map(|(name, key)| (name.as_str().into(), *key))
        .collect();
    exports.sort();
    exports.dedup();
    let mut imports: Vec<(Box<str>, tsox_model::ImportUse)> = table
        .import_uses
        .iter()
        .map(|(specifier, usage)| (specifier.as_str().into(), *usage))
        .collect();
    imports.sort_by(|a, b| a.0.cmp(&b.0));
    tsox_model::SemanticModel {
        flow_facts: table.flow_facts.iter().cloned().collect(),
        node_type: table.node_types.iter().cloned().collect(),
        node_symbol: table.node_symbols.iter().cloned().collect(),
        const_values: table.const_values.iter().cloned().collect(),
        module_info: tsox_model::ModuleInfo { exports, imports },
        ..Default::default()
    }
}

fn crashed(ctx: &UnitContext<'_>) -> UnitOutput {
    let message = messages::internal_checker_error();
    UnitOutput {
        model: tsox_model::SemanticModel::default(),
        // A crashed unit's reads are unknown, so it claims all of them.
        options_read: crate::check_options::OptionReads::all(),
        diagnostics: vec![Diagnostic {
            severity: Severity::Error,
            code: message.code,
            message: message.text,
            file_name: ctx.file_name.to_string(),
            start: 0,
            end: 0,
        }],
        incomplete: true,
        incomplete_reasons: vec![IncompleteReason::CheckerCrashed],
        exports: Vec::new(),
        export_names: Vec::new(),
        exports_complete: false,
        ambient: Vec::new(),
        umd_namespaces: Vec::new(),
        unresolved_names: Vec::new(),
        return_reads: Vec::new(),
        augmentations: Vec::new(),
        globals: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_an_incomplete_output_and_the_worker_keeps_working() {
        let mut worker = Worker::new();
        let ctx = UnitContext::new(FileId::ROOT, "boom.ts", "const n: number = 1;");

        let crashed = worker
            .contained(&ctx, |_| panic!("checker bug"))
            .expect("a panic is an output, not an error");
        assert!(crashed.incomplete);
        assert_eq!(crashed.diagnostics.len(), 1);
        assert_eq!(crashed.diagnostics[0].code.to_string(), "TSR9101");
        assert_eq!(crashed.diagnostics[0].file_name, "boom.ts");

        let after = worker.check_unit(&ctx).unwrap();
        assert!(!after.incomplete);
        assert!(after.diagnostics.is_empty(), "{after:?}");
    }

    #[test]
    fn the_unit_model_carries_sorted_exports_and_import_uses_and_is_deterministic() {
        let source = r#"
            import { a } from "./value";
            import type { T } from "./types";
            export const z: number = a();
            let t: T;
            export const b: number = 1;
        "#;
        let run = || {
            let ctx =
                UnitContext::new(FileId::ROOT, "m.ts", source).with_module(ModuleKey::builtin());
            Worker::new().check_unit(&ctx).unwrap().model
        };
        let model = run();
        let names: Vec<&str> = model
            .module_info
            .exports
            .iter()
            .map(|(n, _)| &**n)
            .collect();
        assert_eq!(names, ["b", "z"]);
        let imports: Vec<_> = model
            .module_info
            .imports
            .iter()
            .map(|(s, u)| (&**s, *u))
            .collect();
        assert_eq!(
            imports,
            [
                ("./types", tsox_model::ImportUse::TypeOnly),
                ("./value", tsox_model::ImportUse::Value)
            ]
        );
        assert_eq!(model.stable_hash(), run().stable_hash());
    }

    #[test]
    fn the_unit_model_holds_enum_member_values() {
        let source =
            "export const enum E { A, B = 5, C, S = \"x\", H = 1.5 }\nenum F { Z = 1 << 3 }\n";
        let run = || {
            let ctx =
                UnitContext::new(FileId::ROOT, "e.ts", source).with_module(ModuleKey::builtin());
            Worker::new().check_unit(&ctx).unwrap().model
        };
        let model = run();
        let mut values: Vec<_> = model.const_values.values().cloned().collect();
        values.sort_by_key(|v| format!("{v:?}"));
        let numbers: Vec<f64> = model
            .const_values
            .values()
            .filter_map(|v| v.as_f64())
            .collect();
        for expected in [0.0, 5.0, 6.0, 1.5, 8.0] {
            assert!(numbers.contains(&expected), "{expected} in {numbers:?}");
        }
        assert!(
            model
                .const_values
                .values()
                .any(|v| *v == tsox_model::ConstValue::String("x".into()))
        );
        assert_eq!(model.const_values.len(), 6);
        assert_eq!(model.stable_hash(), run().stable_hash());
    }

    #[test]
    fn the_unit_model_names_what_references_refer_to() {
        let source = "export function f(): number { return 1; }\nexport const a: number = f() + f();\nconst b = 1;\n";
        let run = || {
            let ctx =
                UnitContext::new(FileId::ROOT, "s.ts", source).with_module(ModuleKey::builtin());
            Worker::new().check_unit(&ctx).unwrap().model
        };
        let model = run();
        let f = model
            .module_info
            .exports
            .iter()
            .find(|(n, _)| &**n == "f")
            .unwrap()
            .1;
        assert_eq!(model.node_symbol.len(), 2, "{:?}", model.node_symbol);
        assert!(model.node_symbol.values().all(|key| *key == f));
        let indexes: Vec<u32> = model.node_symbol.keys().map(|k| k.index).collect();
        assert_eq!(indexes, [0, 1]);
        assert_eq!(model.stable_hash(), run().stable_hash());
    }

    #[test]
    fn the_unit_model_holds_declaration_types() {
        let source =
            "export const n: number = 1;\nexport function f(x: any): number { return 1; }\n";
        let run = || {
            let ctx =
                UnitContext::new(FileId::ROOT, "t.ts", source).with_module(ModuleKey::builtin());
            Worker::new().check_unit(&ctx).unwrap().model
        };
        let model = run();
        assert_eq!(model.node_type.len(), 2, "{:?}", model.node_type);
        let trusts: Vec<_> = model.node_type.values().map(|f| f.trust).collect();
        assert!(trusts.contains(&tsox_model::Trust::Trusted));
        assert!(trusts.contains(&tsox_model::Trust::Unchecked));
        assert_eq!(model.stable_hash(), run().stable_hash());
    }

    #[test]
    fn the_unit_model_holds_narrowed_reads() {
        let source = "export function f(x: string | number): string {\n  if (typeof x === \"string\") { return x; }\n  return String(x);\n}\n";
        let run = || {
            let ctx =
                UnitContext::new(FileId::ROOT, "n.ts", source).with_module(ModuleKey::builtin());
            Worker::new().check_unit(&ctx).unwrap().model
        };
        let model = run();
        assert!(!model.flow_facts.is_empty(), "{:?}", model.flow_facts);
        assert_eq!(model.stable_hash(), run().stable_hash());
    }

    #[test]
    fn a_unit_reports_the_options_it_read_and_no_others() {
        use crate::check_options::OptionField;
        let reads = |source: &str| {
            let ctx =
                UnitContext::new(FileId::ROOT, "r.ts", source).with_module(ModuleKey::builtin());
            Worker::new().check_unit(&ctx).unwrap().options_read
        };
        let plain = reads("export const a: number = 1;");
        let with_class = reads("class A { m() {} } class B extends A { m() {} } export {};");
        assert!(!plain.contains(OptionField::NoImplicitOverride));
        assert!(with_class.contains(OptionField::NoImplicitOverride));
        // Flipping an option the unit never read cannot change its answer.
        assert!(plain.agree(
            &crate::check_options::CheckOptions::typescript_defaults(),
            &crate::check_options::CheckOptions {
                no_implicit_override: true,
                ..crate::check_options::CheckOptions::typescript_defaults()
            }
        ));
    }

    // Every read of a compiler option goes through `CheckContext::option`, which records it. A
    // direct `ctx.options.<field>` read would be missing from `options_read`, and a unit would
    // then be skipped after a change to an option it depends on.
    #[test]
    fn no_checker_code_reads_an_option_without_recording_it() {
        fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, hits);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.ends_with("context.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (n, line) in text.lines().enumerate() {
                        if line.contains("ctx.options.") && !line.trim_start().starts_with("//") {
                            hits.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bridge"),
            &mut hits,
        );
        assert!(hits.is_empty(), "unrecorded option reads: {hits:?}");
    }
}
