//! The library cache is on by default, in a per-user directory, and never changes what a run
//! prints. These run the binary with its environment set by hand, since the workspace's own
//! cargo config turns the cache off for every other test.
#![cfg(all(feature = "snapshot", target_os = "linux"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tsox-cli-cache-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn project(root: &Path) -> PathBuf {
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"strict":true,"target":"es2020","lib":["es2020"]},"files":["main.ts"]}"#,
    )
    .unwrap();
    fs::write(
        root.join("main.ts"),
        "const a: number[] = [1, 2];\nconst s: string = a.map(x => x + 1);\n",
    )
    .unwrap();
    root.join("tsconfig.json")
}

// The binary with nothing in its environment that the person did not put there.
fn run(config: &Path, home: &Path, extra_args: &[&str], cache_env: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tsox"));
    command
        .arg("--project")
        .arg(config)
        .args(extra_args)
        .env_remove("TSOX_LIB_CACHE_DIR")
        .env_remove("XDG_CACHE_HOME")
        .env("HOME", home);
    if let Some(value) = cache_env {
        command.env("TSOX_LIB_CACHE_DIR", value);
    }
    command.output().unwrap()
}

fn entries(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "unit"))
                .count()
        })
        .unwrap_or(0)
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn it_is_on_by_default_in_the_users_cache_directory_and_changes_no_output() {
    let root = scratch("default");
    let config = project(&root);
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let cache = home.join(".cache/tsox/lib-cache");

    let off = run(&config, &home, &["--no-lib-cache"], None);
    assert!(!cache.exists(), "--no-lib-cache makes no directory");

    let cold = run(&config, &home, &[], None);
    assert!(
        entries(&cache) > 10,
        "a first run fills it ({})",
        entries(&cache)
    );
    let warm = run(&config, &home, &[], None);
    assert_eq!(text(&cold), text(&off));
    assert_eq!(text(&warm), text(&off));
    assert_eq!(cold.status.code(), off.status.code());

    // Only this user can use it.
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(&cache).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn xdg_cache_home_and_the_environment_variable_choose_the_directory_and_off_switches_it_off() {
    let root = scratch("choose");
    let config = project(&root);
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();

    // The environment variable names a directory.
    let chosen = root.join("chosen");
    let first = run(&config, &home, &[], Some(chosen.to_str().unwrap()));
    assert!(entries(&chosen) > 10);
    assert!(
        !home.join(".cache").exists(),
        "nothing went to the default place"
    );

    // `off` switches it off, and so does the flag, whatever the variable says.
    let off_dir = root.join("never");
    let off = run(&config, &home, &[], Some("off"));
    assert!(!home.join(".cache").exists());
    let flagged = run(
        &config,
        &home,
        &["--no-lib-cache"],
        Some(off_dir.to_str().unwrap()),
    );
    assert!(!off_dir.exists());
    assert_eq!(text(&first), text(&off));
    assert_eq!(text(&flagged), text(&off));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_cache_directory_that_cannot_be_used_is_an_error_only_when_it_was_asked_for() {
    let root = scratch("unusable");
    let config = project(&root);
    // HOME is a file, so the default directory cannot be made: the run goes on without it.
    let home_file = root.join("home-is-a-file");
    fs::write(&home_file, b"x").unwrap();
    let plain = run(&config, &root, &["--no-lib-cache"], None);
    let by_default = run(&config, &home_file, &[], None);
    assert_eq!(text(&by_default), text(&plain));
    assert_eq!(by_default.status.code(), plain.status.code());

    // Asked for explicitly, the same failure is reported and is not hidden.
    let explicit = run(
        &config,
        &root,
        &["--lib-cache", home_file.join("sub").to_str().unwrap()],
        None,
    );
    assert_eq!(explicit.status.code(), Some(2));
    assert!(
        text(&explicit).contains("cannot use the library cache"),
        "{}",
        text(&explicit)
    );
    let _ = fs::remove_dir_all(&root);
}
