// The native host: the one place in this crate's public surface that stands for the file
// system. Code that is moved onto `tsox_base::host::Host` takes this in a terminal and an
// editor, and `MemoryHost` in a test or a browser.

use std::io;
use std::path::Path;

use tsox_base::host::Host;

/// Reads the real file system. It keeps no cache of its own; the CLI's disk cache is a
/// separate `Host` wrapper that is added when the CLI is moved onto this trait.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsHost;

impl Host for FsHost {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<std::path::PathBuf>> {
        Ok(std::fs::read_dir(path)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect())
    }

    fn real_path(&self, path: &Path) -> std::path::PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_a_real_file_and_reports_a_missing_one() {
        let dir = std::env::temp_dir().join(format!("tsox-host-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ts");
        std::fs::write(&file, "let a = 1;").unwrap();
        let host = FsHost;
        assert_eq!(host.read_to_string(&file).unwrap(), "let a = 1;");
        assert!(host.is_file(&file) && host.is_dir(&dir));
        assert!(host.read_to_string(&dir.join("missing.ts")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
