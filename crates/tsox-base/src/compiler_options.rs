use crate::check_options::CheckOptions;

/// A TypeScript script target as understood by the project configuration layer.
///
/// Unknown values are retained rather than silently coerced. A later validation/resolution
/// phase can report the correct config diagnostic without losing the user's spelling.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ScriptTarget {
    Es3,
    Es5,
    Es2015,
    Es2016,
    Es2017,
    Es2018,
    Es2019,
    Es2020,
    Es2021,
    Es2022,
    Es2023,
    Es2024,
    Es2025,
    EsNext,
    Unknown(String),
}

impl ScriptTarget {
    /// Parses a `target` string from tsconfig, case-insensitively.
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "es3" => Self::Es3,
            "es5" => Self::Es5,
            "es6" | "es2015" => Self::Es2015,
            "es2016" => Self::Es2016,
            "es2017" => Self::Es2017,
            "es2018" => Self::Es2018,
            "es2019" => Self::Es2019,
            "es2020" => Self::Es2020,
            "es2021" => Self::Es2021,
            "es2022" => Self::Es2022,
            "es2023" => Self::Es2023,
            "es2024" => Self::Es2024,
            "es2025" => Self::Es2025,
            "esnext" | "latest" => Self::EsNext,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

impl Default for ScriptTarget {
    fn default() -> Self {
        // TypeScript's compiler default when `target` is omitted.
        Self::Es5
    }
}

/// The normalized compiler environment for a project.
///
/// `lib: None` means TypeScript's target-dependent default library set. `Some(vec![])` is
/// deliberately distinct: it means the config explicitly selected an empty library list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveCompilerOptions {
    /// Options consumed by the per-file semantic checker.
    pub check: CheckOptions,
    /// Output language target. Defaults to ES5 when not specified.
    pub target: ScriptTarget,
    /// Explicit standard-library names, normalized to lowercase. `None` selects the
    /// target-dependent default libraries; an empty list remains an explicit empty selection.
    pub lib: Option<Vec<String>>,
    /// `noLib`: do not add the default library files.
    pub no_lib: bool,
    /// `skipLibCheck`: suppress diagnostics in declaration files. This does not mean that
    /// declarations are omitted from the semantic environment.
    pub skip_lib_check: bool,
}

impl EffectiveCompilerOptions {
    /// The historical tsox comparison profile used by callers without a tsconfig.
    pub fn comparison_harness_defaults() -> Self {
        Self {
            check: CheckOptions::comparison_harness_defaults(),
            ..Self::default()
        }
    }
}

impl Default for EffectiveCompilerOptions {
    fn default() -> Self {
        Self {
            check: CheckOptions::typescript_defaults(),
            target: ScriptTarget::default(),
            lib: None,
            no_lib: false,
            skip_lib_check: false,
        }
    }
}
