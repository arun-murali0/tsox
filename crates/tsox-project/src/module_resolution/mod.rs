// Project-level module resolution: which files a project contains, which of them import
// which, and in what order they can be checked. The checker itself still sees one file
// at a time; this layer only decides what to hand it, and when.
//
// Path and specifier resolution is oxc_resolver's job. What lives here is what the
// resolver does not do: the graph over the results, cycle detection, dependency order,
// change detection and the parallel driver.
pub mod check;
mod config;
mod demand_dom;
mod discovery;
mod error;
mod graph;
#[cfg(feature = "snapshot")]
mod library_cache;
pub mod metadata;
mod partial;
mod resolver;
#[cfg(feature = "snapshot")]
mod scan_cache;
mod schedule;

pub use tsox_base::compiler_options::{EffectiveCompilerOptions, ScriptTarget};
pub use tsox_base::standard_lib::{
    StandardLibraryError, StandardLibraryFile, StandardLibrarySet, TYPESCRIPT_LIB_VERSION,
};

pub use check::{
    FileOutcome, FileReport, ProjectReport, check_project, check_project_bounded,
    check_project_cancellable,
};
pub use config::{ConfigError, Json, ProjectConfig, parse_jsonc};
pub use demand_dom::project_needs_dom_libraries;
pub use discovery::{ModuleRequest, ModuleRequestKind, ModuleScan, scan_module_requests};
pub use error::ModuleError;
pub use graph::{ModuleEdge, ModuleGraph};
#[cfg(feature = "snapshot")]
pub use library_cache::{DiskLibraryUnitCache, LibraryUnitCache, install_library_unit_cache};
#[allow(unused_imports)]
pub use metadata::FileState;
pub use metadata::{FileFingerprint, content_hash};
pub use partial::{PartialRow, PartialSummary, ReasonKind, summarize_partial};
pub use resolver::{ModuleResolver, ResolveMode};
pub use schedule::{Cyclic, Finished, UnitOutcome, run_units, unit_dependencies};
