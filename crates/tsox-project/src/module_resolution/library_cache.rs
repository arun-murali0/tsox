//! An opt-in cache of checked bundled library files.
//!
//! Checking the ES library files is most of what a small project costs once the DOM files are
//! deferred. Their unit outputs are small, arena-free data (see `UnitSnapshot`), so a run can
//! store them and a later run read them back instead of checking the files again.
//!
//! What may be stored is decided in `check_file`, and the rule is narrow on purpose: only a
//! library file, and only one whose unit asked nothing of the rest of the project. Its
//! diagnostics are stored with it, so a cache hit gives the unit exactly as it was checked. A
//! unit that asked nothing cannot have been influenced by what the project declares, so its output depends on its text and the settings below alone. The key
//! is a hash of all of those and of the running program, so a rebuilt checker never reads what
//! an older one wrote. Anything that does not decode is checked as usual and written again.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Entries not written for this long are removed when a cache is swept. A rebuilt checker
/// keys its entries differently, so the old ones are never read again and would pile up.
const MAX_ENTRY_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// What a sweep leaves behind at most. When the entries that are left come to more, the
/// oldest are removed first. A cache that is on by default must not grow without bound, and
/// a developer who rebuilds the checker all day leaves a new set of entries after each build.
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// A sweep reads the metadata of every entry, which for a full cache costs more than a small
/// project's whole check. So a cache is swept at most this often, and the time of the last
/// sweep is the modification time of a marker file in the directory.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const SWEEP_MARKER: &str = ".swept";

/// A file left by a process that stopped between writing an entry and renaming it into place.
const ABANDONED_TEMPORARY_AGE: Duration = Duration::from_secs(60 * 60);
use std::sync::OnceLock;

use tsox_base::keys::ENC_VERSION;
use tsox_base::standard_lib::TYPESCRIPT_LIB_VERSION;
use tsox_checker::unit_snapshot::SNAPSHOT_FORMAT;
use xxhash_rust::xxh3::Xxh3;

/// Where checked library files are kept. Implementations must be safe to call from the
/// threads that check units at the same time.
pub trait LibraryUnitCache: Send + Sync {
    /// The bytes stored under `key`, if any.
    fn load(&self, key: u128) -> Option<Vec<u8>>;
    /// Stores `bytes` under `key`. A failure is not an error: the file is checked next time.
    fn store(&self, key: u128, bytes: &[u8]);
    /// Identity of the program that writes and reads entries: a different value means the
    /// checker may behave differently, so no entry of the other value may be read.
    fn fingerprint(&self) -> u128;
}

static CACHE: OnceLock<Box<dyn LibraryUnitCache>> = OnceLock::new();

/// Installs the process-wide cache. False if one is installed already.
pub fn install_library_unit_cache(cache: Box<dyn LibraryUnitCache>) -> bool {
    CACHE.set(cache).is_ok()
}

pub(super) fn installed() -> Option<&'static dyn LibraryUnitCache> {
    CACHE.get().map(|cache| &**cache)
}

/// What an entry is stored under: everything about a unit that is not an answer from the rest
/// of the project (those are the probes stored inside the entry and checked on every read).
pub(super) fn key(
    cache: &dyn LibraryUnitCache,
    file_name: &str,
    source: &str,
    module: Option<u128>,
    options: &dyn std::fmt::Debug,
) -> u128 {
    let mut hasher = Xxh3::new();
    hasher.update(&cache.fingerprint().to_le_bytes());
    hasher.update(&SNAPSHOT_FORMAT.to_le_bytes());
    hasher.update(&[ENC_VERSION]);
    hasher.update(TYPESCRIPT_LIB_VERSION.as_bytes());
    hasher.update(&module.unwrap_or(0).to_le_bytes());
    hasher.update(format!("{options:?}").as_bytes());
    hasher.update(&(file_name.len() as u64).to_le_bytes());
    hasher.update(file_name.as_bytes());
    hasher.update(&(source.len() as u64).to_le_bytes());
    hasher.update(source.as_bytes());
    hasher.digest128()
}

/// One file per key under a directory. The fingerprint is the size and modification time of
/// the running executable, so rebuilding the program starts a new cache.
pub struct DiskLibraryUnitCache {
    dir: PathBuf,
    fingerprint: u128,
}

impl DiskLibraryUnitCache {
    /// A cache in `dir`, which is created if it is missing.
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        sweep_if_due(&dir, SystemTime::now());
        Ok(Self {
            dir,
            fingerprint: executable_fingerprint(),
        })
    }

    fn path(&self, key: u128) -> PathBuf {
        self.dir.join(format!("{key:032x}.unit"))
    }
}

impl LibraryUnitCache for DiskLibraryUnitCache {
    fn load(&self, key: u128) -> Option<Vec<u8>> {
        fs::read(self.path(key)).ok()
    }

    fn store(&self, key: u128, bytes: &[u8]) {
        // Written beside the final name and renamed into place, so a reader on another
        // thread or in another process sees all of an entry or none of it.
        let path = self.path(key);
        let temporary = path.with_extension(format!("tmp{}", std::process::id()));
        let written = fs::File::create(&temporary).and_then(|mut file| file.write_all(bytes));
        if written.is_ok() && fs::rename(&temporary, &path).is_ok() {
            return;
        }
        let _ = fs::remove_file(&temporary);
    }

    fn fingerprint(&self) -> u128 {
        self.fingerprint
    }
}

/// Sweeps `dir` unless it was swept less than `SWEEP_INTERVAL` before `now`.
fn sweep_if_due(dir: &Path, now: SystemTime) {
    let marker = dir.join(SWEEP_MARKER);
    let last = fs::metadata(&marker).and_then(|meta| meta.modified()).ok();
    // A marker dated in the future (the clock was set back) counts as due: better one sweep
    // too many than a cache that is never swept.
    let due = last
        .and_then(|last| now.duration_since(last).ok())
        .is_none_or(|age| age >= SWEEP_INTERVAL);
    if due {
        remove_stale_entries(dir, now, MAX_TOTAL_BYTES);
        let _ = fs::write(&marker, b"");
    }
}

/// Removes from `dir`:
/// - `.unit` files last written more than `MAX_ENTRY_AGE` before `now`,
/// - then, if the rest come to more than `max_total` bytes, the oldest of them until they do not,
/// - and the temporary files of writers that stopped before finishing.
///
/// Failures are ignored: a file that stays only costs disk space, and one removed while another
/// process reads it is a miss for that process and checked again.
fn remove_stale_entries(dir: &Path, now: SystemTime, max_total: u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        let is_unit = extension == "unit";
        let is_temporary = extension.starts_with("tmp");
        if !is_unit && !is_temporary {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let written = meta.modified().ok();
        let age = written.and_then(|written| now.duration_since(written).ok());
        let limit = if is_unit {
            MAX_ENTRY_AGE
        } else {
            ABANDONED_TEMPORARY_AGE
        };
        if age.is_some_and(|age| age > limit) {
            let _ = fs::remove_file(&path);
        } else if is_unit {
            kept.push((written.unwrap_or(now), meta.len(), path));
        }
    }
    let mut total: u64 = kept.iter().map(|(_, size, _)| size).sum();
    if total > max_total {
        kept.sort_by_key(|(written, _, _)| *written);
        for (_, size, path) in kept {
            if total <= max_total {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= size;
            }
        }
    }
}

fn executable_fingerprint() -> u128 {
    let mut hasher = Xxh3::new();
    if let Ok(path) = std::env::current_exe()
        && let Ok(meta) = fs::metadata(&path)
    {
        hasher.update(&meta.len().to_le_bytes());
        if let Ok(modified) = meta.modified()
            && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            hasher.update(&since.as_nanos().to_le_bytes());
        }
        hasher.update(path.as_os_str().as_encoded_bytes());
    } else {
        // No way to identify the program: a value no other run shares, so nothing is reused.
        hasher.update(&std::process::id().to_le_bytes());
        hasher.update(
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
                .to_le_bytes(),
        );
    }
    hasher.digest128()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsox-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // A file of `size` bytes last written `age` before `now`.
    fn entry(dir: &Path, name: &str, size: usize, now: SystemTime, age: Duration) {
        let file = fs::File::create(dir.join(name)).unwrap();
        file.set_len(size as u64).unwrap();
        file.set_modified(now - age).unwrap();
    }

    #[test]
    fn old_entries_are_removed_and_recent_ones_kept() {
        let dir = scratch("evict");
        fs::write(dir.join("a.unit"), b"x").unwrap();
        fs::write(dir.join("keep.txt"), b"x").unwrap();

        remove_stale_entries(&dir, SystemTime::now(), u64::MAX);
        assert!(dir.join("a.unit").exists());

        remove_stale_entries(
            &dir,
            SystemTime::now() + MAX_ENTRY_AGE + Duration::from_secs(60),
            u64::MAX,
        );
        assert!(!dir.join("a.unit").exists());
        assert!(dir.join("keep.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn over_the_size_limit_the_oldest_entries_go_first() {
        let dir = scratch("cap");
        let now = SystemTime::now();
        entry(&dir, "oldest.unit", 400, now, Duration::from_secs(3000));
        entry(&dir, "middle.unit", 400, now, Duration::from_secs(2000));
        entry(&dir, "newest.unit", 400, now, Duration::from_secs(1000));
        fs::write(dir.join("other.txt"), vec![0u8; 5000]).unwrap();

        // 1200 bytes, limit 900: one has to go, and it is the oldest.
        remove_stale_entries(&dir, now, 900);
        assert!(!dir.join("oldest.unit").exists());
        assert!(dir.join("middle.unit").exists());
        assert!(dir.join("newest.unit").exists());

        // 800 bytes left, limit 500: the next oldest goes, and the newest stays.
        remove_stale_entries(&dir, now, 500);
        assert!(!dir.join("middle.unit").exists());
        assert!(dir.join("newest.unit").exists());

        // Under the limit nothing is touched, and files that are not entries never are.
        remove_stale_entries(&dir, now, 500);
        assert!(dir.join("newest.unit").exists());
        assert!(dir.join("other.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_temporary_file_is_removed_only_once_it_is_clearly_abandoned() {
        let dir = scratch("tmp");
        let now = SystemTime::now();
        entry(&dir, "fresh.tmp123", 10, now, Duration::from_secs(5));
        entry(
            &dir,
            "abandoned.tmp456",
            10,
            now,
            ABANDONED_TEMPORARY_AGE * 2,
        );
        remove_stale_entries(&dir, now, u64::MAX);
        assert!(
            dir.join("fresh.tmp123").exists(),
            "a writer may still be at it"
        );
        assert!(!dir.join("abandoned.tmp456").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sweep_runs_at_most_once_per_interval() {
        let dir = scratch("interval");
        let now = SystemTime::now();
        entry(&dir, "old.unit", 10, now, MAX_ENTRY_AGE * 2);

        // No marker yet: due. The marker is written.
        sweep_if_due(&dir, now);
        assert!(!dir.join("old.unit").exists());
        assert!(dir.join(SWEEP_MARKER).exists());

        // Just swept: an old entry added now is left until the interval has passed.
        entry(&dir, "old2.unit", 10, now, MAX_ENTRY_AGE * 2);
        sweep_if_due(&dir, now + Duration::from_secs(60));
        assert!(
            dir.join("old2.unit").exists(),
            "swept a minute ago, so not again"
        );
        sweep_if_due(&dir, now + SWEEP_INTERVAL + Duration::from_secs(60));
        assert!(!dir.join("old2.unit").exists());

        // A marker dated in the future is treated as due.
        entry(&dir, "old3.unit", 10, now, MAX_ENTRY_AGE * 2);
        let marker = fs::File::options()
            .write(true)
            .open(dir.join(SWEEP_MARKER))
            .unwrap();
        marker
            .set_modified(now + Duration::from_secs(86_400))
            .unwrap();
        sweep_if_due(&dir, now);
        assert!(!dir.join("old3.unit").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
