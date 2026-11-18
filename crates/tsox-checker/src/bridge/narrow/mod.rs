use std::collections::HashMap;

use oxc_ast::ast::{
    AssignmentExpression, AssignmentTarget, BinaryExpression, BinaryOperator, Expression,
    IdentifierReference, LogicalOperator, Program, SimpleAssignmentTarget, StaticMemberExpression,
    UnaryOperator, UpdateExpression,
};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{Scoping, SymbolId};

use crate::arena::{TypeArena, TypeId};
use crate::bridge::context::CheckContext;
use crate::lib_global_types::is_lib_global_type;
use crate::namespace::Resolution;
use crate::types::Type;

// Narrowing by feature. Each file owns one family of rules and says so in its first lines;
// `use super::*` gives them the shared imports above, and the glob re-exports below keep
// every path that was `bridge::narrow::x` working.
mod state;
pub use self::state::*;
mod condition;
pub use self::condition::*;
mod switch;
pub use self::switch::*;
mod forms;
pub use self::forms::*;
mod in_operator;
use self::in_operator::*;
mod instanceof;
use self::instanceof::*;
mod typeof_tags;
use self::typeof_tags::*;
mod literal;
use self::literal::*;
mod truthiness;
pub use self::truthiness::*;
mod predicate;
pub use self::predicate::*;
mod destructure;
pub use self::destructure::*;
mod effects;
pub use self::effects::apply_call_effects;
mod equality;
mod narrowers;
