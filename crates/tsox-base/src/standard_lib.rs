//! Selection and loading of the TypeScript standard-library declaration files.
//!
//! The declaration files are vendored under `lib/typescript-6.0.3/`. The small `.order.json`
//! manifests beside them are generated from the library `/// <reference lib="..." />` graph
//! and preserve TypeScript's dependency-first order for each target's default library set.
//! Explicit `lib` selections are walked from the declarations themselves so subsets such as
//! `lib: ["es2020", "dom"]` do not accidentally acquire the defaults from `es2020.full`.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::compiler_options::{EffectiveCompilerOptions, ScriptTarget};
use crate::reference_directives::{self, DirectiveKind};

/// TypeScript version the bundled declarations were copied from.
pub const TYPESCRIPT_LIB_VERSION: &str = "6.0.3";

const LIB_DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/lib/typescript-6.0.3");

const ES5_ORDER: &str = include_str!("../lib/es5.order.json");
const ES2015_ORDER: &str = include_str!("../lib/es2015.order.json");
const ES2016_ORDER: &str = include_str!("../lib/es2016.order.json");
const ES2017_ORDER: &str = include_str!("../lib/es2017.order.json");
const ES2018_ORDER: &str = include_str!("../lib/es2018.order.json");
const ES2019_ORDER: &str = include_str!("../lib/es2019.order.json");
const ES2020_ORDER: &str = include_str!("../lib/es2020.order.json");
const ES2021_ORDER: &str = include_str!("../lib/es2021.order.json");
const ES2022_ORDER: &str = include_str!("../lib/es2022.order.json");
const ES2023_ORDER: &str = include_str!("../lib/es2023.order.json");
const ES2024_ORDER: &str = include_str!("../lib/es2024.order.json");
const ES2025_ORDER: &str = include_str!("../lib/es2025.order.json");
const ESNEXT_ORDER: &str = include_str!("../lib/esnext.order.json");

/// A selected, vendored TypeScript library declaration file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandardLibraryFile {
    /// The library name without the `lib.` prefix or `.d.ts` suffix. The special name
    /// `default` denotes the `lib.d.ts` compatibility root used for ES3/ES5 defaults.
    pub name: String,
    /// Absolute path to the vendored declaration file.
    pub path: PathBuf,
}

impl StandardLibraryFile {
    /// Reads this declaration file as UTF-8 source.
    pub fn read_source(&self) -> Result<String, StandardLibraryError> {
        fs::read_to_string(&self.path).map_err(|source| StandardLibraryError::Read {
            path: self.path.clone(),
            source,
        })
    }
}

/// The standard-library files for one effective compiler environment, in dependency-first order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandardLibrarySet {
    files: Vec<StandardLibraryFile>,
}

impl StandardLibrarySet {
    /// Selects TypeScript's target-dependent defaults or the explicitly configured `lib` list.
    /// `noLib` suppresses all automatic library inclusion.
    pub fn for_options(options: &EffectiveCompilerOptions) -> Result<Self, StandardLibraryError> {
        if options.no_lib {
            return Ok(Self { files: Vec::new() });
        }

        let files = match &options.lib {
            Some(names) => explicit_libraries(names)?,
            None => default_libraries(&options.target)?,
        };
        Ok(Self { files })
    }

    /// Returns the selected files in dependency-first order.
    pub fn files(&self) -> &[StandardLibraryFile] {
        &self.files
    }

    /// Returns the selected absolute paths in dependency-first order.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Library names that only declare browser and host globals: `lib.dom.d.ts` and the files that
/// extend or accompany it. They are 84% of the bytes of the default ES2020 set and nothing in
/// the ES libraries refers to them.
const DOM_FAMILY: &[&str] = &[
    "dom",
    "dom.iterable",
    "dom.asynciterable",
    "webworker.importscripts",
    "scripthost",
];

/// True for a library file that declares DOM or host globals (see [`DOM_FAMILY`]).
///
/// The `*.full` files, `lib.d.ts` (named `default`) and `lib.es6.d.ts` are included: they hold
/// only `/// <reference lib>` lines, and their DOM references would bring the DOM files back
/// through the module graph. Their ES references are listed on their own in the order manifests.
pub fn is_dom_family_library(name: &str) -> bool {
    DOM_FAMILY.contains(&name) || name.ends_with(".full") || name == "default" || name == "es6"
}

/// True when `name` is a global that only the DOM-family libraries declare: no ES library
/// declares it. A project that never uses such a name, and never declares one, does not need
/// those files to be loaded to get the same diagnostics.
pub fn is_dom_only_global(name: &str) -> bool {
    crate::dom_only_globals::DOM_ONLY_GLOBALS
        .binary_search(&name)
        .is_ok()
}

impl StandardLibrarySet {
    /// This set without the DOM-family files. A file listed in the order manifests keeps its
    /// place, so what is left is still dependency-first.
    pub fn without_dom_family(&self) -> Self {
        Self {
            files: self
                .files
                .iter()
                .filter(|file| !is_dom_family_library(&file.name))
                .cloned()
                .collect(),
        }
    }

    /// True when at least one DOM-family file is part of this set.
    pub fn has_dom_family(&self) -> bool {
        self.files
            .iter()
            .any(|file| is_dom_family_library(&file.name))
    }
}

/// Errors selecting or reading a bundled standard-library declaration.
#[derive(Debug)]
pub enum StandardLibraryError {
    UnknownTarget(String),
    UnknownLibrary(String),
    InvalidOrderManifest(String),
    ReferenceCycle(String),
    Read { path: PathBuf, source: io::Error },
}

impl fmt::Display for StandardLibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTarget(target) => write!(f, "unsupported TypeScript target '{target}'"),
            Self::UnknownLibrary(name) => write!(f, "unknown TypeScript library '{name}'"),
            Self::InvalidOrderManifest(reason) => {
                write!(
                    f,
                    "invalid bundled TypeScript library order manifest: {reason}"
                )
            }
            Self::ReferenceCycle(name) => {
                write!(f, "cyclic TypeScript library reference involving '{name}'")
            }
            Self::Read { path, source } => {
                write!(
                    f,
                    "cannot read TypeScript library {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for StandardLibraryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The directory containing this crate's vendored TypeScript standard-library declarations.
pub fn library_directory() -> PathBuf {
    PathBuf::from(LIB_DIRECTORY)
}

/// Resolves one `/// <reference lib="…" />` name to its declaration file.
///
/// Only the named file is returned. Its own reference directives are followed by module
/// discovery, which records the full dependency closure in the project graph.
pub fn resolve_library_reference(name: &str) -> Result<PathBuf, StandardLibraryError> {
    let normalized = normalize_library_name(name);
    library_path(&normalized)
}

fn default_libraries(
    target: &ScriptTarget,
) -> Result<Vec<StandardLibraryFile>, StandardLibraryError> {
    let manifest = match target {
        ScriptTarget::Es3 | ScriptTarget::Es5 => ES5_ORDER,
        ScriptTarget::Es2015 => ES2015_ORDER,
        ScriptTarget::Es2016 => ES2016_ORDER,
        ScriptTarget::Es2017 => ES2017_ORDER,
        ScriptTarget::Es2018 => ES2018_ORDER,
        ScriptTarget::Es2019 => ES2019_ORDER,
        ScriptTarget::Es2020 => ES2020_ORDER,
        ScriptTarget::Es2021 => ES2021_ORDER,
        ScriptTarget::Es2022 => ES2022_ORDER,
        ScriptTarget::Es2023 => ES2023_ORDER,
        ScriptTarget::Es2024 => ES2024_ORDER,
        ScriptTarget::Es2025 => ES2025_ORDER,
        ScriptTarget::EsNext => ESNEXT_ORDER,
        ScriptTarget::Unknown(target) => {
            return Err(StandardLibraryError::UnknownTarget(target.clone()));
        }
    };

    let names = parse_order_manifest(manifest)?;
    let mut files = Vec::with_capacity(names.len() + 1);
    // TypeScript's ES3/ES5 default root is `lib.d.ts`, whose references are represented by
    // es5.order.json. Keep the root itself in the program as well; it contains directives only.
    if matches!(target, ScriptTarget::Es3 | ScriptTarget::Es5) {
        files.push(StandardLibraryFile {
            name: "default".to_owned(),
            path: library_directory().join("lib.d.ts"),
        });
    }
    for name in names {
        let path = library_path(&name)?;
        files.push(StandardLibraryFile { name, path });
    }
    Ok(files)
}

fn explicit_libraries(names: &[String]) -> Result<Vec<StandardLibraryFile>, StandardLibraryError> {
    let mut files = Vec::new();
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    for name in names {
        visit_library(name, &mut visiting, &mut visited, &mut files)?;
    }
    Ok(files)
}

fn visit_library(
    requested: &str,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    files: &mut Vec<StandardLibraryFile>,
) -> Result<(), StandardLibraryError> {
    let name = normalize_library_name(requested);
    if visited.contains(&name) {
        return Ok(());
    }
    if !visiting.insert(name.clone()) {
        return Err(StandardLibraryError::ReferenceCycle(name));
    }

    let path = library_path(&name)?;
    let source = fs::read_to_string(&path).map_err(|source| StandardLibraryError::Read {
        path: path.clone(),
        source,
    })?;
    for directive in reference_directives::scan(&source) {
        if directive.kind == DirectiveKind::Lib {
            visit_library(&directive.specifier, visiting, visited, files)?;
        }
    }

    visiting.remove(&name);
    visited.insert(name.clone());
    files.push(StandardLibraryFile { name, path });
    Ok(())
}

fn normalize_library_name(name: &str) -> String {
    let mut normalized = name.trim().to_ascii_lowercase();
    if let Some(without_prefix) = normalized.strip_prefix("lib.") {
        normalized = without_prefix.to_owned();
    }
    if let Some(without_suffix) = normalized.strip_suffix(".d.ts") {
        normalized = without_suffix.to_owned();
    }
    normalized
}

fn library_path(name: &str) -> Result<PathBuf, StandardLibraryError> {
    // Library names are generated identifiers, not paths. Reject separators rather than
    // allowing a malformed tsconfig/reference directive to escape the vendored directory.
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'_')
    {
        return Err(StandardLibraryError::UnknownLibrary(name.to_owned()));
    }
    let file_name = format!("lib.{name}.d.ts");
    let path = library_directory().join(file_name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(StandardLibraryError::UnknownLibrary(name.to_owned()))
    }
}

fn parse_order_manifest(source: &str) -> Result<Vec<String>, StandardLibraryError> {
    let trimmed = source.trim();
    let Some(body) = trimmed
        .strip_prefix('[')
        .and_then(|body| body.strip_suffix(']'))
    else {
        return Err(StandardLibraryError::InvalidOrderManifest(
            "expected a JSON string array".to_owned(),
        ));
    };
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut names = Vec::new();
    for item in body.split(',') {
        let item = item.trim();
        let Some(name) = item
            .strip_prefix('"')
            .and_then(|name| name.strip_suffix('"'))
        else {
            return Err(StandardLibraryError::InvalidOrderManifest(format!(
                "invalid library name entry {item:?}"
            )));
        };
        names.push(name.to_owned());
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dom_only_globals_are_sorted_and_cover_the_known_cases() {
        use crate::dom_only_globals::DOM_ONLY_GLOBALS;
        assert!(DOM_ONLY_GLOBALS.windows(2).all(|pair| pair[0] < pair[1]));
        for name in [
            "document",
            "window",
            "HTMLElement",
            "setTimeout",
            "console",
            "URL",
        ] {
            assert!(super::is_dom_only_global(name), "{name}");
        }
        for name in ["Array", "Promise", "Map", "Symbol", "Record", "Date"] {
            assert!(!super::is_dom_only_global(name), "{name}");
        }
    }

    #[test]
    fn deferring_dom_keeps_the_es_files_in_order() {
        let set = StandardLibrarySet::for_options(&options(ScriptTarget::Es2020)).unwrap();
        assert!(set.has_dom_family());
        let lean = set.without_dom_family();
        assert!(!lean.has_dom_family());
        assert!(!lean.is_empty());
        let kept: Vec<&str> = set
            .files()
            .iter()
            .map(|file| file.name.as_str())
            .filter(|name| !super::is_dom_family_library(name))
            .collect();
        let lean_names: Vec<&str> = lean.files().iter().map(|file| file.name.as_str()).collect();
        assert_eq!(kept, lean_names);
    }

    use super::{StandardLibraryError, StandardLibrarySet};
    use crate::compiler_options::{EffectiveCompilerOptions, ScriptTarget};

    fn options(target: ScriptTarget) -> EffectiveCompilerOptions {
        EffectiveCompilerOptions {
            target,
            ..EffectiveCompilerOptions::default()
        }
    }

    fn has_file(set: &StandardLibrarySet, file_name: &str) -> bool {
        set.files()
            .iter()
            .any(|file| file.path.file_name().and_then(|name| name.to_str()) == Some(file_name))
    }

    #[test]
    fn es5_default_uses_the_legacy_default_root_and_its_reference_closure() {
        let set = StandardLibrarySet::for_options(&options(ScriptTarget::Es5)).unwrap();
        assert!(has_file(&set, "lib.d.ts"));
        assert!(has_file(&set, "lib.es5.d.ts"));
        assert!(has_file(&set, "lib.dom.d.ts"));
        assert!(has_file(&set, "lib.scripthost.d.ts"));
        assert!(!has_file(&set, "lib.es2020.d.ts"));
    }

    #[test]
    fn target_selects_the_matching_full_default_library_set() {
        let set = StandardLibrarySet::for_options(&options(ScriptTarget::Es2022)).unwrap();
        assert!(has_file(&set, "lib.es2022.full.d.ts"));
        assert!(has_file(&set, "lib.es2022.d.ts"));
        assert!(has_file(&set, "lib.dom.d.ts"));
        assert!(!has_file(&set, "lib.es2023.d.ts"));
    }

    #[test]
    fn es2015_and_esnext_select_their_version_specific_roots() {
        let es2015 = StandardLibrarySet::for_options(&options(ScriptTarget::Es2015)).unwrap();
        assert!(has_file(&es2015, "lib.es6.d.ts"));
        assert!(!has_file(&es2015, "lib.es2016.full.d.ts"));

        let esnext = StandardLibrarySet::for_options(&options(ScriptTarget::EsNext)).unwrap();
        assert!(has_file(&esnext, "lib.esnext.full.d.ts"));
    }

    #[test]
    fn explicit_libs_walk_only_their_reference_closure() {
        let mut options = options(ScriptTarget::Es2022);
        options.lib = Some(vec!["ES2020".to_owned(), "DOM".to_owned()]);
        let set = StandardLibrarySet::for_options(&options).unwrap();
        assert!(has_file(&set, "lib.es2020.d.ts"));
        assert!(has_file(&set, "lib.dom.d.ts"));
        assert!(!has_file(&set, "lib.es2020.full.d.ts"));
        assert!(!has_file(&set, "lib.es2022.d.ts"));
        assert!(!has_file(&set, "lib.scripthost.d.ts"));
    }

    #[test]
    fn explicit_empty_lib_and_no_lib_disable_default_loading() {
        let mut options = options(ScriptTarget::Es2022);
        options.lib = Some(Vec::new());
        assert!(
            StandardLibrarySet::for_options(&options)
                .unwrap()
                .is_empty()
        );

        options.lib = Some(vec!["es2020".to_owned()]);
        options.no_lib = true;
        assert!(
            StandardLibrarySet::for_options(&options)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unknown_explicit_library_is_not_silently_replaced_with_anything() {
        let mut options = options(ScriptTarget::Es2022);
        options.lib = Some(vec!["not-a-real-lib".to_owned()]);
        assert!(matches!(
            StandardLibrarySet::for_options(&options),
            Err(StandardLibraryError::UnknownLibrary(name)) if name == "not-a-real-lib"
        ));
    }

    #[test]
    fn deferring_dom_drops_the_dom_family_and_the_full_roots_only() {
        let set = StandardLibrarySet::for_options(&options(ScriptTarget::Es2020)).unwrap();
        assert!(set.has_dom_family());
        let lean = set.without_dom_family();
        assert!(!lean.has_dom_family());
        assert!(has_file(&lean, "lib.es5.d.ts"));
        assert!(has_file(&lean, "lib.es2020.bigint.d.ts"));
        assert!(!has_file(&lean, "lib.dom.d.ts"));
        assert!(!has_file(&lean, "lib.es2020.full.d.ts"));
        assert_eq!(lean.files().len() + 6, set.files().len());
    }
}
