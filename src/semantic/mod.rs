mod generics;
pub mod queries;
mod solver;

// Only the helpers still used outside this module. The call checker reaches the rest
// through GenericSolver, which calls into generics.rs directly.
pub(crate) use generics::{
    contains_type_param, resolve_conditional_type, substitute_bound_type_params,
    substitute_type_params,
};
pub use queries::SemanticQueries;
pub(crate) use solver::{GenericInferenceSession, GenericSolver, InferencePriority};
