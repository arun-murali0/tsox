// Problem: the Salsa layer was a cfg-gated module of the root crate, so the one place
// that will grow into the real query layer shared a namespace with the checker it wraps,
// and a query could reach a checker internal that the project driver does not offer.
// Picked: its own crate on top of tsox-project, reading the driver only through what
// module_resolution exports. Salsa is a dependency of this crate alone.
// Cost: a query that needs something the driver keeps private has to ask for it to be
// exported, which is the point: each such request marks a seam to turn into a query.

use tsox_base::keys;
use tsox_checker::unit::CancelToken;
use tsox_project::module_resolution;
use tsox_project::module_resolution::{FileFingerprint, FileReport, content_hash};
use tsox_types::{portable, types};

pub mod incremental;
