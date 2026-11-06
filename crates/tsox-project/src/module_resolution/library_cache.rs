//! An opt-in cache of checked bundled library files.
//!
//! Checking the ES library files is most of what a small project costs once the DOM files are
//! deferred. Their unit outputs are small, arena-free data (see `UnitSnapshot`), so a run can
//! store them and a later run read them back instead of checking the files again.
//!
//! What may be stored is decided in `check_file`, and the rule is narrow on purpose: only a
//! library file, and only one whose unit asked nothing of the rest of the project. Its
//! diagnostics are stored with it, so a cache hit gives the unit exactly as it was checked. A unit that asked nothing cannot have been influenced by what the
//! project declares, so its output depends on its text and the settings below alone. The key
//! is a hash of all of those and of the running program, so a rebuilt checker never reads what
//! an older one wrote. Anything that does not decode is checked as usual and written again.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
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

/// Everything a library unit's output can depend on once it has asked nothing of the project.
pub(super) fn key(
    cache: &dyn LibraryUnitCache,
    source: &str,
    module: Option<u128>,
    options: &dyn std::fmt::Debug,
    collect_global_decls: bool,
) -> u128 {
    let mut hasher = Xxh3::new();
    hasher.update(&cache.fingerprint().to_le_bytes());
    hasher.update(&SNAPSHOT_FORMAT.to_le_bytes());
    hasher.update(&[ENC_VERSION, u8::from(collect_global_decls)]);
    hasher.update(TYPESCRIPT_LIB_VERSION.as_bytes());
    hasher.update(&module.unwrap_or(0).to_le_bytes());
    hasher.update(format!("{options:?}").as_bytes());
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
