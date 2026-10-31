use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use xxhash_rust::xxh3::xxh3_64;

// Coarser than any filesystem timestamp in use (FAT stores two-second ticks).
const RACY_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

// What a single stat call says about a file. Cheap to take and to compare, but only as
// trustworthy as the filesystem's clock: a rewrite that keeps the size and lands in the
// same timestamp tick is invisible to it, the same blind spot `make` has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileFingerprint {
    /// The fingerprint of a file with no copy on disk (an unsaved buffer).
    pub(crate) fn absent() -> Self {
        Self {
            len: 0,
            modified: None,
        }
    }

    pub fn of(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// XXH3 of the file's bytes. Not a security hash; it only has to tell two versions of
/// a file apart, quickly.
pub fn content_hash(bytes: &[u8]) -> u64 {
    xxh3_64(bytes)
}

// A file as it was when the graph read it. The fingerprint is the fast answer and the
// hash is the confirmation: a touched file whose bytes did not change has a new
// fingerprint and the old hash, and is still unchanged.
#[derive(Clone, Debug)]
pub(crate) struct FileState {
    fingerprint: FileFingerprint,
    hash: u64,
    // When the file was read. A file modified within `RACY_WINDOW` of this moment may be
    // rewritten again in the same timestamp tick without its fingerprint changing, so the
    // fingerprint alone is not trusted for it (the "racily clean" case of git's index).
    observed: SystemTime,
    // The text discovery read, kept so the checker does not read the file again. Only a
    // scannable file whose bytes are valid UTF-8 has one; for any other file this is None
    // and the checker reads the disk itself, as it always did. It is the text the hash
    // and the fingerprint describe, so a file edited after the walk is still checked as
    // it was seen, and `is_unchanged` is what says it has moved on.
    text: Option<SourceText>,
}

// The text of one file, shared and cheap to hand out. Debug prints its length, because
// the graph derives Debug and a printed graph should not contain every source file.
#[derive(Clone)]
struct SourceText(Arc<str>);

impl std::fmt::Debug for SourceText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SourceText({} bytes)", self.0.len())
    }
}

impl FileState {
    pub(crate) fn new(fingerprint: FileFingerprint, hash: u64) -> Self {
        Self {
            fingerprint,
            hash,
            observed: SystemTime::now(),
            text: None,
        }
    }

    pub(super) fn with_text(mut self, text: Option<Arc<str>>) -> Self {
        self.text = text.map(SourceText);
        self
    }

    pub(super) fn text(&self) -> Option<&str> {
        self.text.as_ref().map(|text| &*text.0)
    }

    // An unreadable file is reported as changed. Calling it unchanged would keep stale
    // results for a file that may be gone; calling it changed only costs a re-check.
    pub(crate) fn is_unchanged(&self, path: &Path) -> bool {
        let Ok(current) = FileFingerprint::of(path) else {
            return false;
        };
        if current == self.fingerprint && self.fingerprint_is_trustworthy() {
            return true;
        }
        fs::read(path).is_ok_and(|bytes| content_hash(&bytes) == self.hash)
    }

    // A fingerprint that matches proves the file is the same only when the file was already
    // old when it was read: then a later write cannot have landed in the same timestamp
    // tick. A file with no modification time, one modified in the future, or one modified
    // within the window before it was read is confirmed by its bytes instead. The cost is a
    // hash of recently edited files, which is what an edit-save-check loop touches anyway.
    fn fingerprint_is_trustworthy(&self) -> bool {
        match self.fingerprint.modified {
            Some(modified) => self
                .observed
                .duration_since(modified)
                .is_ok_and(|age| age > RACY_WINDOW),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileFingerprint, FileState, content_hash};

    #[test]
    fn a_same_length_rewrite_in_the_same_tick_is_not_mistaken_for_no_change() {
        let dir = std::env::temp_dir().join(format!("tsox-racy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.ts");
        std::fs::write(&path, b"const a = 1;").unwrap();
        let state = FileState::new(
            FileFingerprint::of(&path).unwrap(),
            content_hash(b"const a = 1;"),
        );
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        // Same length, same modification time: the stat call cannot tell these apart.
        std::fs::write(&path, b"const a = 2;").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(FileFingerprint::of(&path).unwrap(), state.fingerprint);
        assert!(!state.is_unchanged(&path), "the bytes changed");
        // An untouched file is still recognised, by its bytes when its stamp is recent.
        std::fs::write(&path, b"const a = 1;").unwrap();
        let state = FileState::new(
            FileFingerprint::of(&path).unwrap(),
            content_hash(b"const a = 1;"),
        );
        assert!(state.is_unchanged(&path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_hash_separates_contents_of_equal_length() {
        assert_ne!(content_hash(b"const a = 1;"), content_hash(b"const a = 2;"));
        assert_eq!(content_hash(b"same"), content_hash(b"same"));
    }
}
