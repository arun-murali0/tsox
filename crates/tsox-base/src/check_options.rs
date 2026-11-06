/// Options consumed by semantic checking for one source file.
///
/// `Default` deliberately preserves tsox's historical comparison-harness profile. That
/// profile enables `noUncheckedIndexedAccess`, which is stricter than TypeScript's ordinary
/// defaults. Project configuration must use [`CheckOptions::typescript_defaults`] as its base
/// and then apply the options from `tsconfig.json`; it must not use this compatibility default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckOptions {
    /// TypeScript's umbrella strictness switch.
    pub strict: bool,
    /// `noImplicitAny`, enabled by `strict` unless explicitly overridden.
    pub no_implicit_any: bool,
    /// `noImplicitThis`, enabled by `strict` unless explicitly overridden.
    pub no_implicit_this: bool,
    /// `strictNullChecks`, enabled by `strict` unless explicitly overridden.
    pub strict_null_checks: bool,
    /// `strictFunctionTypes`, enabled by `strict` unless explicitly overridden.
    pub strict_function_types: bool,
    /// `strictBindCallApply`, enabled by `strict` unless explicitly overridden.
    pub strict_bind_call_apply: bool,
    /// `strictPropertyInitialization`, enabled by `strict` unless explicitly overridden.
    pub strict_property_initialization: bool,
    /// `alwaysStrict`, enabled by `strict` unless explicitly overridden.
    pub always_strict: bool,
    /// `strictBuiltinIteratorReturn`, enabled by `strict` unless explicitly overridden.
    pub strict_builtin_iterator_return: bool,
    /// `arr[i]` includes `undefined` (`noUncheckedIndexedAccess`). This is not implied by
    /// `strict`; it is enabled only by an explicit compiler option in a TypeScript project.
    pub no_unchecked_indexed_access: bool,
    /// Suppress unreachable-code diagnostics. TypeScript's omitted-option behavior is to
    /// suppress these diagnostics; an explicit `allowUnreachableCode: false` turns them on.
    pub allow_unreachable_code: bool,
    /// A catch variable is `unknown` rather than `any`; defaults to the value of `strict`.
    pub use_unknown_in_catch_variables: bool,
    /// `exactOptionalPropertyTypes`; independent of `strict`.
    pub exact_optional_property_types: bool,
    /// `noPropertyAccessFromIndexSignature`; independent of `strict`.
    pub no_property_access_from_index_signature: bool,
    /// `noImplicitOverride`; independent of `strict`.
    pub no_implicit_override: bool,
    /// `noImplicitReturns`; independent of `strict`.
    pub no_implicit_returns: bool,
    /// `noFallthroughCasesInSwitch`; independent of `strict`.
    pub no_fallthrough_cases_in_switch: bool,
}

impl CheckOptions {
    /// The option defaults TypeScript uses when no `tsconfig.json` compiler options are set.
    ///
    /// This intentionally differs from `Default`, which is retained as tsox's historical
    /// comparison-harness profile for direct/single-file callers.
    pub const fn typescript_defaults() -> Self {
        Self {
            strict: false,
            no_implicit_any: false,
            no_implicit_this: false,
            strict_null_checks: false,
            strict_function_types: false,
            strict_bind_call_apply: false,
            strict_property_initialization: false,
            always_strict: false,
            strict_builtin_iterator_return: false,
            no_unchecked_indexed_access: false,
            allow_unreachable_code: true,
            use_unknown_in_catch_variables: false,
            exact_optional_property_types: false,
            no_property_access_from_index_signature: false,
            no_implicit_override: false,
            no_implicit_returns: false,
            no_fallthrough_cases_in_switch: false,
        }
    }

    /// The pre-existing tsox comparison profile. Keep this explicit at call sites that
    /// deliberately compare against `tsc` with the project's additional strict flags.
    pub const fn comparison_harness_defaults() -> Self {
        Self {
            no_unchecked_indexed_access: true,
            allow_unreachable_code: false,
            ..Self::typescript_defaults()
        }
    }
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self::comparison_harness_defaults()
    }
}
