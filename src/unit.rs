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

    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = Some(cancel);
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
}

/// The exports of one `declare module "x"` block (see `UnitOutput`).
#[derive(Clone, Debug)]
pub struct AmbientExports {
    pub specifier: String,
    pub exports: Vec<PortableDecl>,
    pub export_names: Vec<(String, DeclKey)>,
    pub complete: bool,
    /// Global names (see `UnitOutput::globals`) whose declaration could not be carried over.
    pub unmodelled: Vec<String>,
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
    pub(crate) fn run_unit<R>(
        &mut self,
        _ctx: &UnitContext<'_>,
        f: impl FnOnce(&mut TypeArena) -> R,
    ) -> R
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
                ctx.cancel.as_ref(),
                ctx.module,
                ctx.imports,
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
                            names_only: false,
                            augmentation: false,
                        })
                        .collect(),
                    umd_namespaces: table.umd_namespaces,
                    unresolved_names: table.unresolved_names,
                    globals: if table.global_names.is_empty() && table.global_unmodelled.is_empty()
                    {
                        Vec::new()
                    } else {
                        vec![AmbientExports {
                            specifier: String::new(),
                            complete: true,
                            unmodelled: table.global_unmodelled,
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

fn crashed(ctx: &UnitContext<'_>) -> UnitOutput {
    let message = messages::internal_checker_error();
    UnitOutput {
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
}
