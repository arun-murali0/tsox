//! Document URIs and project paths.
//!
//! The project layer keys files by canonical path (`IncrementalProject::set_buffer`
//! canonicalizes, and the graph is built from canonical entries). A URI from the editor is
//! turned into the same form here, so a buffer lands on the file the graph knows. A file
//! that does not exist on disk (an untitled or not yet saved document) cannot be
//! canonicalized; it keeps its absolute path.

use std::path::{Path, PathBuf};

use tower_lsp_server::ls_types::Uri;

/// The path of a `file:` URI, canonical when the file exists. `None` for any other scheme.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.scheme().as_str().eq_ignore_ascii_case("file") {
        return None;
    }
    let path = uri.to_file_path()?.into_owned();
    Some(canonical_or_same(&path))
}

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    Uri::from_file_path(path)
}

/// The path as the project graph spells it (the same rule `IncrementalProject` uses): the
/// canonical path when the file exists, else its canonical folder with the file name kept,
/// else the path as given. A buffer for a file that is not saved yet must land on the key the
/// project gives it.
pub fn canonical_or_same(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    match (path.parent().map(std::fs::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn other_schemes_have_no_path() {
        let uri = Uri::from_str("untitled:Untitled-1").unwrap();
        assert_eq!(uri_to_path(&uri), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_file_keeps_its_path() {
        let uri = Uri::from_str("file:///no/such/dir/a.ts").unwrap();
        assert_eq!(uri_to_path(&uri), Some(PathBuf::from("/no/such/dir/a.ts")));
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_file_resolves_through_symlinks() {
        let dir = std::env::temp_dir().join(format!("tsox-lsp-paths-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.ts");
        std::fs::write(&real, "").unwrap();
        let link = dir.join("link.ts");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let uri = path_to_uri(&link).unwrap();
        assert_eq!(
            uri_to_path(&uri),
            Some(std::fs::canonicalize(&real).unwrap())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_new_file_in_an_existing_folder_gets_the_canonical_folder() {
        let dir = std::env::temp_dir().join(format!("tsox-lsp-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let canonical_dir = std::fs::canonicalize(&dir).unwrap();
        let got = canonical_or_same(&dir.join("not-saved.ts"));
        assert_eq!(got, canonical_dir.join("not-saved.ts"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_path_round_trips_through_a_uri() {
        let uri = path_to_uri(Path::new("/no/such/with space.ts")).unwrap();
        assert_eq!(
            uri_to_path(&uri),
            Some(PathBuf::from("/no/such/with space.ts"))
        );
    }
}
