//! Where the library cache lives, and whether a run uses one.
//!
//! The cache (see `tsox::DiskLibraryUnitCache`) keeps what checking the bundled library files
//! and a project's dependency declaration files produced, so a later run reads it instead of
//! doing the work again. It is on unless it is switched off, and it is chosen in this order:
//!
//! 1. `--no-lib-cache`: off.
//! 2. `--lib-cache <dir>`: that directory. A directory that cannot be used is an error, since
//!    the person asked for it.
//! 3. `TSOX_LIB_CACHE_DIR`: `off` (or `0`, `no`, `false`, `none`, or empty) is off, anything
//!    else is a directory, with the same rule as the flag.
//! 4. The platform's cache directory (see [`default_dir`]). A directory that cannot be used
//!    here is not an error: the run goes on without a cache, as if it had been switched off.
//!
//! Output never depends on the cache: it is the same cold, warm or off, and a damaged or
//! foreign entry is a miss.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What a run does about the cache.
#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    Off,
    /// Asked for by flag or environment. A failure to use it is reported.
    Explicit(PathBuf),
    /// The platform default. A failure to use it is not reported.
    Default(PathBuf),
}

/// The values of environment variables, so a test does not touch the real environment.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

pub fn choose(args: &[String], env: Env<'_>) -> Choice {
    if args.iter().any(|arg| arg == "--no-lib-cache") {
        return Choice::Off;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--lib-cache") {
        return match args.get(index + 1) {
            Some(dir) => Choice::Explicit(PathBuf::from(dir)),
            None => Choice::Off,
        };
    }
    if let Some(value) = env("TSOX_LIB_CACHE_DIR") {
        return if is_off(&value) {
            Choice::Off
        } else {
            Choice::Explicit(PathBuf::from(value))
        };
    }
    default_dir(env).map_or(Choice::Off, Choice::Default)
}

fn is_off(value: &OsString) -> bool {
    let text = value.to_string_lossy();
    let text = text.trim().to_ascii_lowercase();
    matches!(text.as_str(), "" | "off" | "0" | "no" | "false" | "none")
}

/// The platform's per-user cache directory for this cache, or `None` when it cannot be told
/// (no home directory).
///
/// - Windows: `%LOCALAPPDATA%\tsox\lib-cache`
/// - macOS: `~/Library/Caches/tsox/lib-cache`
/// - elsewhere: `$XDG_CACHE_HOME/tsox/lib-cache` when that is an absolute path, else
///   `~/.cache/tsox/lib-cache`
pub fn default_dir(env: Env<'_>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        env(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if cfg!(windows) {
        return absolute("LOCALAPPDATA").map(|dir| dir.join("tsox").join("lib-cache"));
    }
    if cfg!(target_os = "macos") {
        return absolute("HOME").map(|home| home.join("Library/Caches/tsox/lib-cache"));
    }
    absolute("XDG_CACHE_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".cache")))
        .map(|dir| dir.join("tsox").join("lib-cache"))
}

/// Makes `dir` exist and be one only this user can use, or says why it cannot be trusted.
///
/// What is read back from the cache is data about types that the checker believes, so the
/// default directory must not be somewhere another user could have put entries. It is created
/// with mode 0700; if it exists it must be a directory owned by the running user (a file this
/// process creates in it has that owner, which is how it is told without asking for the user
/// id), and a mode that lets others in is tightened. On Windows the per-user directories it
/// lives in are already private.
#[cfg_attr(not(feature = "snapshot"), allow(dead_code))]
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let refuse =
            |why: &str| std::io::Error::new(std::io::ErrorKind::PermissionDenied, why.to_string());
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let meta = std::fs::metadata(dir)?;
        if !meta.is_dir() {
            return Err(refuse("is not a directory"));
        }
        let probe = dir.join(format!(".owner-check-{}", std::process::id()));
        let made = std::fs::File::create(&probe).and_then(|_| std::fs::metadata(&probe));
        let _ = std::fs::remove_file(&probe);
        if made?.uid() != meta.uid() {
            return Err(refuse("is owned by another user"));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn the_flag_beats_the_environment_and_the_environment_beats_the_default() {
        let env = env_of(&[
            ("TSOX_LIB_CACHE_DIR", "/from/env"),
            ("HOME", "/home/u"),
            ("XDG_CACHE_HOME", "/xdg"),
        ]);
        assert_eq!(
            choose(
                &args(&["--project", "t.json", "--lib-cache", "/from/flag"]),
                &env
            ),
            Choice::Explicit(PathBuf::from("/from/flag"))
        );
        assert_eq!(
            choose(&args(&["--project", "t.json"]), &env),
            Choice::Explicit(PathBuf::from("/from/env"))
        );
        let env = env_of(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/xdg")]);
        assert!(matches!(
            choose(&args(&["--project", "t.json"]), &env),
            Choice::Default(_)
        ));
    }

    #[test]
    fn it_can_be_switched_off_by_flag_or_environment() {
        let env = env_of(&[("HOME", "/home/u"), ("TSOX_LIB_CACHE_DIR", "/from/env")]);
        // The flag that turns it off beats a directory named in the environment, and also a
        // directory named by `--lib-cache` in the same command.
        assert_eq!(choose(&args(&["--no-lib-cache"]), &env), Choice::Off);
        assert_eq!(
            choose(&args(&["--lib-cache", "/d", "--no-lib-cache"]), &env),
            Choice::Off
        );
        for value in ["off", "OFF", "0", "no", "false", "none", "", "  "] {
            let pairs = [("HOME", "/home/u"), ("TSOX_LIB_CACHE_DIR", value)];
            let env = env_of(&pairs);
            assert_eq!(
                choose(&args(&["--project", "t"]), &env),
                Choice::Off,
                "{value:?}"
            );
        }
        // A flag with no directory after it is no cache, not a guess.
        assert_eq!(choose(&args(&["--lib-cache"]), &env), Choice::Off);
    }

    #[test]
    fn no_home_means_no_default_cache() {
        let env = env_of(&[]);
        assert_eq!(choose(&args(&["--project", "t"]), &env), Choice::Off);
        // A relative home is not trusted either.
        let env = env_of(&[("HOME", "relative/home")]);
        assert_eq!(choose(&args(&["--project", "t"]), &env), Choice::Off);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_default_follows_xdg_then_home() {
        let env = env_of(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/xdg")]);
        assert_eq!(
            default_dir(&env),
            Some(PathBuf::from("/xdg/tsox/lib-cache"))
        );
        let env = env_of(&[("HOME", "/home/u")]);
        assert_eq!(
            default_dir(&env),
            Some(PathBuf::from("/home/u/.cache/tsox/lib-cache"))
        );
        // The specification says a relative XDG_CACHE_HOME is ignored.
        let env = env_of(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "rel")]);
        assert_eq!(
            default_dir(&env),
            Some(PathBuf::from("/home/u/.cache/tsox/lib-cache"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_private_directory_is_created_private_and_a_loose_one_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("tsox-privdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("a/b/lib-cache");
        ensure_private_dir(&dir).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700, "a directory we own is tightened");

        // No probe file is left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        // Something that is not a directory is refused.
        let file = root.join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(ensure_private_dir(&file).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
