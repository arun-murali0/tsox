// Problem: the driver that orders files, runs them in parallel and settles import cycles
// sat in the same crate as the checker it drives, and the `module-resolution` feature was
// a cfg on half of the root crate, so a build without it still compiled the rest around
// the gap.
// Picked: the graph, the configuration, the scheduler and check_project are one crate
// that depends on tsox-checker and not the other way round. The feature now decides only
// whether the facade pulls this crate in, and the checker cannot name anything here.
// Cost: the checker's cycle rounds and second pass still live in check.rs next to the
// scheduler, because they read the same export tables; they move only when those tables
// become queries.

use tsox_base::check_options::CheckOptions;
use tsox_base::{
    diagnostics, error, fxhash, keys, project, reference_directives, source_depth, stack_guard,
};
use tsox_checker::{bridge, unit};
use tsox_types::{diagnostic_messages, portable, types};

pub mod module_resolution;
pub mod scc;
