// The compiler options the checker's own passes read (E5). Everything else in a tsconfig is
// for resolution and file selection and never reaches a unit.
//
// Problem: the checker had one fixed behavior. The comparison harness runs tsc with a stricter
// set than `--strict` alone (`allowUnreachableCode: false`, `noUncheckedIndexedAccess`), and
// a project that says otherwise in its tsconfig got reports it had switched off.
// Picked: the default is that fixed behavior, so a unit given no options, and a project whose
// tsconfig leaves a key out (the harness writes an empty one), check exactly as before. A key
// the tsconfig sets explicitly overrides it.
// Cost: a key left out is not derived from `strict` the way tsc derives it, so
// `useUnknownInCatchVariables` follows only an explicit setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckOptions {
    /// `arr[i]` includes `undefined` (tsc's `noUncheckedIndexedAccess`). On by default.
    pub no_unchecked_indexed_access: bool,
    /// Unreachable code is not an error (tsc's `allowUnreachableCode: true`). Off by default.
    pub allow_unreachable_code: bool,
    /// A catch variable is `unknown` and not `any` (`useUnknownInCatchVariables`). Off by default.
    pub use_unknown_in_catch_variables: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            no_unchecked_indexed_access: true,
            allow_unreachable_code: false,
            use_unknown_in_catch_variables: false,
        }
    }
}
