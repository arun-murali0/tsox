// Problem: the walk over one file (declare, bodies, publish) and the unit that wraps it
// for a worker were modules of the same crate as the project driver, so the driver could
// call a checker internal directly and a change inside bridge could break scheduling
// without a signature changing.
// Picked: bridge and unit are one crate, and what the layers above may use is what this
// file lists. `unit` stays beside `bridge` because they call each other: a unit runs
// check_file, and check_file reads the cancel token and the export table the unit types.
// Cost: the items the project layer needs (FileEnv, Checked, ExportTable and the rest)
// are `pub` now, where they were `pub(crate)`. The facade crate still decides what the
// outside world sees.

use tsox_base::{
    check_options, diagnostic_codes, diagnostics, error, fxhash, keys, lib_global_types,
    reference_directives, source_depth, stack_guard,
};
use tsox_types::{
    arena, diagnostic_messages, namespace, portable, semantic, stable_walk, subtyping, symbol_map,
    type_annotation, type_display, types,
};

pub mod bridge;
// The flow graph and the reachability rules read the Oxc AST, so they live in `bridge`, the
// one place that does; the rest of the crate sees only what they return. These paths stay.
pub use bridge::{flow, reachability};
#[cfg(feature = "snapshot")]
pub mod probe_log;
pub mod unit;
#[cfg(feature = "snapshot")]
pub mod unit_snapshot;
