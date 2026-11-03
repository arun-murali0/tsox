// Problem: the type system (arena, types, subtyping, generics, namespace, annotation
// resolution) shared a crate with the AST walk that uses it, so a relation could be
// written against a checker detail and the arena could not be built or tested alone.
// Picked: these modules are one crate that knows no statement or expression of the
// checker. They stay together because they call each other in both directions (arena
// and subtyping, types and arena, namespace and type_annotation), so a split inside
// them would need a trait at every one of those calls.
// Cost: type_annotation and namespace still read Oxc's AST, so this crate depends on
// oxc_ast. What it must not do is read a `CheckContext`; that type lives one crate up.

// The foundations are named here so a path like `crate::fxhash::FxHashMap` in a moved
// module resolves as it did before the split.
use tsox_base::{diagnostic_codes, fxhash, keys, stable_hash, stack_guard};

pub mod arena;
pub mod diagnostic_messages;
pub mod explain;
pub mod namespace;
#[allow(dead_code)]
pub mod portable;
pub mod semantic;
#[allow(dead_code)]
pub mod stable_walk;
pub mod subtyping;
pub mod symbol_map;
pub mod type_annotation;
pub mod type_display;
pub mod types;
pub mod variance;
