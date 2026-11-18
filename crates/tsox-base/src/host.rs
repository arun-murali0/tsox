// Problem: reading a file, listing what exists and caching a result are done with `std::fs`
// in three crates, so the checker can not run in a browser worker or in a test with no disk
// without a feature flag in each place, and nothing says what the outside world is allowed to
// provide.
// Picked: one small trait, `Host`, that every outside capability goes through, and an in-memory
// implementation that is the test double and the WASM default. A native implementation lives
// in a crate that may touch the file system (`tsox-project::host::FsHost`).
// Cost: the move is partial. The tsconfig reader, the graph's source reads, `include` selection
// and the `package.json` lookups go through it; `oxc_resolver`, the library loader and the disk
// cache still touch the file system (see docs/architecture/new-architecture-progress.md).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// What the checker, the language server and the emitter may ask of the world. Implementations
/// are shared between threads, so reads take `&self`.
pub trait Host: Send + Sync {
    /// The text of a file, or why it could not be read.
    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    /// Whether `path` names a file.
    fn is_file(&self, path: &Path) -> bool;

    /// Whether `path` names a directory.
    fn is_dir(&self, path: &Path) -> bool;

    /// The entries of a directory as full paths, in no particular order, or why it could not be
    /// listed. Whether an entry is a directory is asked with `is_dir`, which follows links.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;

    /// The path with links and `..` resolved, which names a directory the same way however it
    /// was reached. A host with no links returns the path as given.
    fn real_path(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }

    /// A stored result for `key`, if the host keeps a cache. The default keeps none.
    fn cache_get(&self, _key: &str) -> Option<Arc<[u8]>> {
        None
    }

    /// Stores a result under `key`. Hosts without a cache ignore it.
    fn cache_put(&self, _key: &str, _bytes: Arc<[u8]>) {}
}

/// A host over files held in memory: the test double, and what a browser caller uses. Paths
/// are looked up as given (no normalization), and directories exist because a file is below them.
#[derive(Debug, Default)]
pub struct MemoryHost {
    files: RwLock<BTreeMap<PathBuf, String>>,
    cache: RwLock<BTreeMap<String, Arc<[u8]>>>,
}

impl MemoryHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a file.
    pub fn set(&self, path: impl Into<PathBuf>, text: impl Into<String>) {
        self.files
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(path.into(), text.into());
    }

    /// Removes a file; whether it existed.
    pub fn remove(&self, path: &Path) -> bool {
        self.files
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(path)
            .is_some()
    }
}

impl Host for MemoryHost {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.display().to_string()))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .any(|file| file.starts_with(path) && file != path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let files = self.files.read().unwrap_or_else(|e| e.into_inner());
        let mut entries: Vec<PathBuf> = Vec::new();
        for file in files.keys() {
            let Ok(below) = file.strip_prefix(path) else {
                continue;
            };
            if let Some(first) = below.components().next() {
                let entry = path.join(first);
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
        }
        if entries.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                path.display().to_string(),
            ));
        }
        Ok(entries)
    }

    fn cache_get(&self, key: &str) -> Option<Arc<[u8]>> {
        self.cache
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    fn cache_put(&self, key: &str, bytes: Arc<[u8]>) {
        self.cache
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_read_back_and_a_missing_one_is_not_found() {
        let host = MemoryHost::new();
        host.set("/p/a.ts", "export {}");
        assert_eq!(
            host.read_to_string(Path::new("/p/a.ts")).unwrap(),
            "export {}"
        );
        let err = host.read_to_string(Path::new("/p/b.ts")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_directory_lists_its_files_and_subdirectories_once() {
        let host = MemoryHost::new();
        host.set("/p/a.ts", "");
        host.set("/p/src/b.ts", "");
        host.set("/p/src/c.ts", "");
        let mut entries = host.read_dir(Path::new("/p")).unwrap();
        entries.sort();
        assert_eq!(entries, [PathBuf::from("/p/a.ts"), PathBuf::from("/p/src")]);
        assert!(host.read_dir(Path::new("/nothing")).is_err());
    }

    #[test]
    fn a_directory_exists_when_a_file_is_below_it() {
        let host = MemoryHost::new();
        host.set("/p/src/a.ts", "");
        assert!(host.is_dir(Path::new("/p")));
        assert!(host.is_dir(Path::new("/p/src")));
        assert!(!host.is_dir(Path::new("/p/src/a.ts")));
        assert!(host.is_file(Path::new("/p/src/a.ts")));
        assert!(!host.is_file(Path::new("/p/src")));
    }

    #[test]
    fn the_cache_round_trips_and_removal_is_reported() {
        let host = MemoryHost::new();
        assert!(host.cache_get("k").is_none());
        host.cache_put("k", Arc::from(&b"v"[..]));
        assert_eq!(&*host.cache_get("k").unwrap(), b"v");
        host.set("/x", "1");
        assert!(host.remove(Path::new("/x")));
        assert!(!host.remove(Path::new("/x")));
    }
}
