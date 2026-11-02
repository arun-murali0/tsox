use std::path::{Path, PathBuf};

use crate::file_id::FileId;
use crate::fxhash::FxHashMap;

// The identity half of a project, and nothing more: a path maps to one FileId for the
// life of the project. Parsing, semantic data, exports and query results can be layered
// on top later without making a path the key that types are identified by, which would
// tie type identity to a filesystem.
//
// Callers pass one consistent path form (absolute and normalized, say). Paths are not
// canonicalized here, because that would make handing out an id a filesystem operation,
// and in-memory and WASM inputs have no filesystem to ask.
#[derive(Debug, Default)]
pub struct ProjectFiles {
    by_path: FxHashMap<PathBuf, FileId>,
    paths: Vec<PathBuf>,
}

impl ProjectFiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// The id for `path`, assigning the next free one the first time it is seen.
    pub fn intern<P: Into<PathBuf>>(&mut self, path: P) -> FileId {
        let path = path.into();
        if let Some(&id) = self.by_path.get(&path) {
            return id;
        }
        let index = u32::try_from(self.paths.len()).expect("project contains too many files");
        let id = FileId::new(index);
        self.by_path.insert(path.clone(), id);
        self.paths.push(path);
        id
    }

    pub fn id(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.paths.get(id.index() as usize).map(PathBuf::as_path)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectFiles;
    use std::path::Path;

    #[test]
    fn file_ids_are_stable_for_repeated_paths() {
        let mut files = ProjectFiles::new();
        let first = files.intern("src/a.ts");
        let second = files.intern("src/b.ts");
        let first_again = files.intern("src/a.ts");

        assert_eq!(first, first_again);
        assert_ne!(first, second);
        assert_eq!(files.len(), 2);
        assert_eq!(files.path(first), Some(Path::new("src/a.ts")));
        assert_eq!(files.id(Path::new("src/b.ts")), Some(second));
        assert_eq!(files.id(Path::new("src/missing.ts")), None);
    }

    #[test]
    fn ids_are_assigned_in_order_starting_at_zero() {
        let mut files = ProjectFiles::new();
        assert!(files.is_empty());
        assert_eq!(files.intern("a.ts").index(), 0);
        assert_eq!(files.intern("b.ts").index(), 1);
    }
}
