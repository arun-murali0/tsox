use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModuleError {
    #[error("cannot resolve `{specifier}` from {}: {reason}", .from.display())]
    Unresolved {
        specifier: String,
        from: PathBuf,
        reason: String,
    },
    #[error("could not read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("project entry must be an absolute path: {}", .0.display())]
    RelativeEntry(PathBuf),
}
