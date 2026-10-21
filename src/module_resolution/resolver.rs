use std::path::{Path, PathBuf};

use oxc_resolver::{
    ResolveOptions, Resolver, TsconfigDiscovery, TsconfigOptions, TsconfigReferences,
};

use super::error::ModuleError;

// oxc_resolver with the options a TypeScript project needs, and its error type turned
// into ModuleError so the dependency does not appear in the public API.
//
// The resolver caches directory listings and package.json reads, so one instance is
// built per project and shared by reference, including across the threads that
// check_project uses.
pub struct ModuleResolver {
    resolver: Resolver,
    // The same options with the `require` condition where `resolver` has `import`, for a
    // request that is a `require` call. See `ResolveMode`.
    require_resolver: Resolver,
    reads_tsconfig: bool,
}

/// Which condition a package's `exports` map is read under. An `import` statement matches
/// the `import` condition; a `require` call and `import x = require()` match `require`.
/// Under NodeNext a `.cts` file is CommonJS, so its imports are `Require` too. The nearest
/// `package.json`'s `type` is not consulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveMode {
    Import,
    Require,
}

impl ModuleResolver {
    pub fn new() -> Self {
        Self {
            resolver: Resolver::new(options(ResolveMode::Import)),
            require_resolver: Resolver::new(options(ResolveMode::Require)),
            reads_tsconfig: false,
        }
    }

    /// Like `new`, and also reads the project's `tsconfig.json`, so `paths`, `baseUrl`,
    /// `extends` chains and project `references` decide where a specifier points, as they
    /// do for TypeScript. `config_file` should be an absolute path. A specifier the
    /// configuration does not map is resolved exactly as `new` would resolve it.
    pub fn with_tsconfig(config_file: impl Into<PathBuf>) -> Self {
        let config_file = config_file.into();
        let with_config = |mode| {
            let mut options = options(mode);
            options.tsconfig = Some(TsconfigDiscovery::Manual(TsconfigOptions {
                config_file: config_file.clone(),
                references: TsconfigReferences::Auto,
            }));
            Resolver::new(options)
        };
        Self {
            resolver: with_config(ResolveMode::Import),
            require_resolver: with_config(ResolveMode::Require),
            reads_tsconfig: true,
        }
    }

    /// Whether this resolver applies a `tsconfig.json`, that is, was made by `with_tsconfig`.
    pub fn reads_tsconfig(&self) -> bool {
        self.reads_tsconfig
    }

    /// Resolves `specifier` as written in `from`, the importing file.
    pub fn resolve_from_file(&self, from: &Path, specifier: &str) -> Result<PathBuf, ModuleError> {
        self.resolve_from_file_as(ResolveMode::Import, from, specifier)
    }

    /// Like `resolve_from_file`, reading a package's `exports` under `mode`.
    pub fn resolve_from_file_as(
        &self,
        mode: ResolveMode,
        from: &Path,
        specifier: &str,
    ) -> Result<PathBuf, ModuleError> {
        let resolver = self.for_mode(mode);
        let found = resolver
            .resolve_file(from, specifier)
            .map(|resolution| resolution.path().to_path_buf());
        self.with_types_fallback(resolver, from, specifier, found)
    }

    fn for_mode(&self, mode: ResolveMode) -> &Resolver {
        match mode {
            ResolveMode::Import => &self.resolver,
            ResolveMode::Require => &self.require_resolver,
        }
    }

    /// Like `resolve_from_file`, but looks for declaration files, which is what an
    /// `import type` needs: a package that ships only types has no JavaScript entry for
    /// the plain resolver to find.
    pub fn resolve_dts_from_file(
        &self,
        from: &Path,
        specifier: &str,
    ) -> Result<PathBuf, ModuleError> {
        self.resolve_dts_from_file_as(ResolveMode::Import, from, specifier)
    }

    /// Like `resolve_dts_from_file`, reading a package's `exports` under `mode`.
    pub fn resolve_dts_from_file_as(
        &self,
        mode: ResolveMode,
        from: &Path,
        specifier: &str,
    ) -> Result<PathBuf, ModuleError> {
        let resolver = self.for_mode(mode);
        let found = resolver
            .resolve_dts(from, specifier)
            .map(|resolution| resolution.path().to_path_buf());
        self.with_types_fallback(resolver, from, specifier, found)
    }

    // A package that ships JavaScript and no declarations of its own is typed by
    // `@types/<name>`, which is where TypeScript looks next. So a package specifier that
    // resolved to a JavaScript file, or to nothing, is tried as its `@types` package, and
    // that wins when it exists. A relative specifier never is.
    fn with_types_fallback(
        &self,
        resolver: &Resolver,
        from: &Path,
        specifier: &str,
        found: Result<PathBuf, oxc_resolver::ResolveError>,
    ) -> Result<PathBuf, ModuleError> {
        let wants_types = match &found {
            Ok(path) => is_javascript(path),
            Err(_) => true,
        };
        if wants_types {
            if let Some(types) = types_specifier(specifier) {
                if let Ok(resolution) = resolver.resolve_dts(from, &types) {
                    return Ok(resolution.path().to_path_buf());
                }
            }
        }
        found.map_err(|error| unresolved(from, specifier, &error))
    }
}

fn is_javascript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "mjs" | "cjs" | "jsx")
    )
}

// `lodash/fp` is `@types/lodash/fp`; `@scope/name` is `@types/scope__name`. None for a
// relative or absolute specifier, which no `@types` package stands for.
fn types_specifier(specifier: &str) -> Option<String> {
    if specifier.starts_with('.') || specifier.starts_with('/') || specifier.starts_with("@types/")
    {
        return None;
    }
    let (name, rest) = match specifier.strip_prefix('@') {
        Some(scoped) => {
            let (scope, tail) = scoped.split_once('/')?;
            let (name, rest) = match tail.split_once('/') {
                Some((name, rest)) => (name, format!("/{rest}")),
                None => (tail, String::new()),
            };
            (format!("{scope}__{name}"), rest)
        }
        None => match specifier.split_once('/') {
            Some((name, rest)) => (name.to_string(), format!("/{rest}")),
            None => (specifier.to_string(), String::new()),
        },
    };
    Some(format!("@types/{name}{rest}"))
}

impl Default for ModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}

fn unresolved(from: &Path, specifier: &str, error: &oxc_resolver::ResolveError) -> ModuleError {
    ModuleError::Unresolved {
        specifier: specifier.to_owned(),
        from: from.to_path_buf(),
        reason: error.to_string(),
    }
}

// `types` comes first among the conditions and main fields so a package's declaration
// entry wins over its JavaScript one. The extension aliases are TypeScript's NodeNext
// rule: `./a.js` in source means `./a.ts` on disk, because the emitted file is the one
// that gets named.
fn options(mode: ResolveMode) -> ResolveOptions {
    let strings =
        |items: &[&str]| -> Vec<String> { items.iter().map(|item| (*item).to_owned()).collect() };
    ResolveOptions {
        extensions: strings(&[".ts", ".tsx", ".mts", ".cts", ".d.ts", ".js", ".jsx"]),
        extension_alias: vec![
            (".js".to_owned(), strings(&[".ts", ".tsx", ".d.ts", ".js"])),
            (".jsx".to_owned(), strings(&[".tsx", ".jsx"])),
            (".mjs".to_owned(), strings(&[".mts", ".d.mts", ".mjs"])),
            (".cjs".to_owned(), strings(&[".cts", ".d.cts", ".cjs"])),
        ],
        condition_names: match mode {
            ResolveMode::Import => strings(&["types", "import", "node"]),
            ResolveMode::Require => strings(&["types", "require", "node"]),
        },
        main_fields: strings(&["types", "typings", "module", "main"]),
        ..ResolveOptions::default()
    }
}
