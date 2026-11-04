// Problem: the checker's foundations (a file's identity, the stable keys and hashes, the
// diagnostic types, the stack and bracket guards) were modules of one crate beside the
// arena, so nothing stopped one of them from reaching into the type system. FileId made
// that concrete: it lived in types.rs, which sits on the arena, so keys and the project
// file table could not be used without pulling the arena in.
// Picked: this crate holds what needs nothing from the type system and is the bottom of
// the workspace. FileId moved here; types.rs re-exports it, so every `types::FileId` path
// is unchanged.
// Cost: a module added here must not name arena, types or subtyping. The compiler holds
// that line now, where before only review did.

pub mod check_options;
pub mod compiler_options;
pub mod diagnostic_codes;
pub mod diagnostic_view;
pub mod diagnostics;
pub mod error;
pub mod file_id;
pub mod fxhash;
pub mod keys;
pub mod lib_global_types;
pub mod line_index;
pub mod project;
pub mod reference_directives;
#[cfg(feature = "scratchpad")]
pub mod scratchpad;
pub mod source_depth;
pub mod stable_hash;
pub mod stack_guard;
pub mod standard_lib;
