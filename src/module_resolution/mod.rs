// Project-level module resolution: which files a project contains, which of them import
// which, and in what order they can be checked. The checker itself still sees one file
// at a time; this layer only decides what to hand it, and when.
//
// Path and specifier resolution is oxc_resolver's job. What lives here is what the
// resolver does not do: the graph over the results, cycle detection, dependency order,
// change detection and the parallel driver.
mod check;
mod discovery;
mod error;
mod graph;
mod metadata;
mod partial;
mod resolver;

pub use check::{FileOutcome, FileReport, ProjectReport, check_project, check_project_bounded};
pub use discovery::{ModuleRequest, ModuleRequestKind, ModuleScan, scan_module_requests};
pub use error::ModuleError;
pub use graph::{ModuleEdge, ModuleGraph};
pub use metadata::{FileFingerprint, content_hash};
pub use partial::{PartialRow, PartialSummary, ReasonKind, summarize_partial};
pub use resolver::ModuleResolver;
