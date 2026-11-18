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
    /// `module` is one of the ECMAScript module formats (`es2015`, `es2020`, `es2022`,
    /// `esnext`): `import x = require()` (TS1202) and `export =` (TS1203) are not allowed in a
    /// non-declaration file. Unset or any other `module` leaves both alone.
    pub module_is_es: bool,
    /// `moduleDetection: "force"`: every file that is not a declaration file is a module, even
    /// with no `import` or `export`, so its top-level names are its own and not global.
    pub module_detection_force: bool,
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
            module_is_es: false,
            module_detection_force: false,
        }
    }

    /// The pre-existing tsox comparison profile. Keep this explicit at call sites that
    /// deliberately compare against `tsc` with the project's additional strict flags.
    pub const fn comparison_harness_defaults() -> Self {
        Self {
            no_unchecked_indexed_access: true,
            no_property_access_from_index_signature: true,
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

/// One field of `CheckOptions`, named so a read can be recorded (see `OptionReads`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OptionField {
    Strict,
    NoImplicitAny,
    NoImplicitThis,
    StrictNullChecks,
    StrictFunctionTypes,
    StrictBindCallApply,
    StrictPropertyInitialization,
    AlwaysStrict,
    StrictBuiltinIteratorReturn,
    NoUncheckedIndexedAccess,
    AllowUnreachableCode,
    UseUnknownInCatchVariables,
    ExactOptionalPropertyTypes,
    NoPropertyAccessFromIndexSignature,
    NoImplicitOverride,
    NoImplicitReturns,
    NoFallthroughCasesInSwitch,
    ModuleIsEs,
    ModuleDetectionForce,
}

impl OptionField {
    pub const ALL: [OptionField; 19] = [
        OptionField::Strict,
        OptionField::NoImplicitAny,
        OptionField::NoImplicitThis,
        OptionField::StrictNullChecks,
        OptionField::StrictFunctionTypes,
        OptionField::StrictBindCallApply,
        OptionField::StrictPropertyInitialization,
        OptionField::AlwaysStrict,
        OptionField::StrictBuiltinIteratorReturn,
        OptionField::NoUncheckedIndexedAccess,
        OptionField::AllowUnreachableCode,
        OptionField::UseUnknownInCatchVariables,
        OptionField::ExactOptionalPropertyTypes,
        OptionField::NoPropertyAccessFromIndexSignature,
        OptionField::NoImplicitOverride,
        OptionField::NoImplicitReturns,
        OptionField::NoFallthroughCasesInSwitch,
        OptionField::ModuleIsEs,
        OptionField::ModuleDetectionForce,
    ];

    /// The tsconfig spelling, for reports.
    pub fn name(self) -> &'static str {
        match self {
            OptionField::Strict => "strict",
            OptionField::NoImplicitAny => "noImplicitAny",
            OptionField::NoImplicitThis => "noImplicitThis",
            OptionField::StrictNullChecks => "strictNullChecks",
            OptionField::StrictFunctionTypes => "strictFunctionTypes",
            OptionField::StrictBindCallApply => "strictBindCallApply",
            OptionField::StrictPropertyInitialization => "strictPropertyInitialization",
            OptionField::AlwaysStrict => "alwaysStrict",
            OptionField::StrictBuiltinIteratorReturn => "strictBuiltinIteratorReturn",
            OptionField::NoUncheckedIndexedAccess => "noUncheckedIndexedAccess",
            OptionField::AllowUnreachableCode => "allowUnreachableCode",
            OptionField::UseUnknownInCatchVariables => "useUnknownInCatchVariables",
            OptionField::ExactOptionalPropertyTypes => "exactOptionalPropertyTypes",
            OptionField::NoPropertyAccessFromIndexSignature => "noPropertyAccessFromIndexSignature",
            OptionField::NoImplicitOverride => "noImplicitOverride",
            OptionField::NoImplicitReturns => "noImplicitReturns",
            OptionField::NoFallthroughCasesInSwitch => "noFallthroughCasesInSwitch",
            OptionField::ModuleIsEs => "module (es)",
            OptionField::ModuleDetectionForce => "moduleDetection: force",
        }
    }
}

impl CheckOptions {
    /// The value of one option. Checker code reads options through `CheckContext::option`, which
    /// records the read; this is the plain lookup under it.
    pub const fn get(&self, field: OptionField) -> bool {
        match field {
            OptionField::Strict => self.strict,
            OptionField::NoImplicitAny => self.no_implicit_any,
            OptionField::NoImplicitThis => self.no_implicit_this,
            OptionField::StrictNullChecks => self.strict_null_checks,
            OptionField::StrictFunctionTypes => self.strict_function_types,
            OptionField::StrictBindCallApply => self.strict_bind_call_apply,
            OptionField::StrictPropertyInitialization => self.strict_property_initialization,
            OptionField::AlwaysStrict => self.always_strict,
            OptionField::StrictBuiltinIteratorReturn => self.strict_builtin_iterator_return,
            OptionField::NoUncheckedIndexedAccess => self.no_unchecked_indexed_access,
            OptionField::AllowUnreachableCode => self.allow_unreachable_code,
            OptionField::UseUnknownInCatchVariables => self.use_unknown_in_catch_variables,
            OptionField::ExactOptionalPropertyTypes => self.exact_optional_property_types,
            OptionField::NoPropertyAccessFromIndexSignature => {
                self.no_property_access_from_index_signature
            }
            OptionField::NoImplicitOverride => self.no_implicit_override,
            OptionField::NoImplicitReturns => self.no_implicit_returns,
            OptionField::NoFallthroughCasesInSwitch => self.no_fallthrough_cases_in_switch,
            OptionField::ModuleIsEs => self.module_is_es,
            OptionField::ModuleDetectionForce => self.module_detection_force,
        }
    }
}

/// The set of options a unit read, as a bit set. A unit whose reads do not include an option that
/// changed has the same answer as before, which is what lets an option change re-check only the
/// units that depend on it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct OptionReads(u32);

impl OptionReads {
    pub const fn none() -> Self {
        OptionReads(0)
    }
    /// Every option: what a unit with unknown reads claims, so it is never skipped wrongly.
    pub const fn all() -> Self {
        OptionReads((1 << OptionField::ALL.len()) - 1)
    }
    pub fn insert(&mut self, field: OptionField) {
        self.0 |= 1 << field as u32;
    }
    pub fn contains(self, field: OptionField) -> bool {
        self.0 & (1 << field as u32) != 0
    }
    pub fn bits(self) -> u32 {
        self.0
    }
    pub fn fields(self) -> impl Iterator<Item = OptionField> {
        OptionField::ALL
            .into_iter()
            .filter(move |f| self.contains(*f))
    }
    /// True when `a` and `b` agree on every option this set contains.
    pub fn agree(self, a: &CheckOptions, b: &CheckOptions) -> bool {
        self.fields().all(|field| a.get(field) == b.get(field))
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn every_field_is_readable_and_names_are_unique() {
        let mut names = std::collections::HashSet::new();
        let on = CheckOptions {
            strict: true,
            no_implicit_any: true,
            no_implicit_this: true,
            strict_null_checks: true,
            strict_function_types: true,
            strict_bind_call_apply: true,
            strict_property_initialization: true,
            always_strict: true,
            strict_builtin_iterator_return: true,
            no_unchecked_indexed_access: true,
            allow_unreachable_code: true,
            use_unknown_in_catch_variables: true,
            exact_optional_property_types: true,
            no_property_access_from_index_signature: true,
            no_implicit_override: true,
            no_implicit_returns: true,
            no_fallthrough_cases_in_switch: true,
            module_is_es: true,
            module_detection_force: true,
        };
        for field in OptionField::ALL {
            assert!(names.insert(field.name()));
            assert!(on.get(field), "{field:?}");
            assert!(
                !CheckOptions::typescript_defaults().get(field)
                    || field == OptionField::AllowUnreachableCode
            );
        }
    }

    #[test]
    fn reads_agree_only_on_the_fields_they_contain() {
        let a = CheckOptions::typescript_defaults();
        let mut b = a;
        b.no_implicit_override = true;
        let mut reads = OptionReads::none();
        reads.insert(OptionField::StrictNullChecks);
        assert!(reads.agree(&a, &b));
        reads.insert(OptionField::NoImplicitOverride);
        assert!(!reads.agree(&a, &b));
    }
}
