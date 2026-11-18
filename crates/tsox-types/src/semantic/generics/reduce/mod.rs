//! One file per type operator the checker reduces, and the one contract they all answer with.
//!
//! A reducer rewrites an operator whose operands are now known (`keyof { a: 1 }` is `"a"`). It says
//! what happened instead of returning a bare type: `Reduced` when the answer no longer depends on
//! a type parameter, `Deferred` when it still does (the operator is rebuilt and waits for a
//! binding), and `TooComplex` when a limit cut the work short, so the type in it is the error type
//! and the cut is recorded by reason (`stack_guard::CutReason`). Nothing becomes `any` silently.
use super::*;
use crate::stack_guard::take_cut_reasons;

mod conditional;
mod keyof_index;
mod mapped;
mod record;
mod template;
mod variadic;
pub use self::conditional::*;
pub use self::keyof_index::*;
pub use self::mapped::*;
pub use self::record::*;
pub use self::template::*;
pub use self::variadic::*;

/// How one reduction ended.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Reduction {
    Reduced(TypeId),
    Deferred(TypeId),
    TooComplex(TypeId),
}

impl Reduction {
    pub fn type_id(self) -> TypeId {
        match self {
            Reduction::Reduced(id) | Reduction::Deferred(id) | Reduction::TooComplex(id) => id,
        }
    }
}

/// An operator application with its operands already rewritten.
pub enum Operator {
    Mapped(crate::types::MappedType),
    Keyof(TypeId),
    IndexedAccess(TypeId, TypeId),
    StringMapping(crate::types::StringCase, TypeId),
    Template(Vec<String>, Vec<TypeId>),
    Variadic(Vec<crate::types::TupleElement>),
}

impl Operator {
    /// The operator's name, matching the file that reduces it. A cut in `reduce_operator` is
    /// reported against it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn name(&self) -> &'static str {
        match self {
            Operator::Mapped(_) => "mapped",
            Operator::Keyof(_) => "keyof",
            Operator::IndexedAccess(..) => "indexed_access",
            Operator::StringMapping(..) => "string_mapping",
            Operator::Template(..) => "template",
            Operator::Variadic(_) => "variadic",
        }
    }
}

/// Reduces one operator and classifies the result. A cut while it ran makes the answer
/// `TooComplex` and is put back for the unit to report; otherwise the answer is `Deferred` while
/// it still mentions a type parameter and `Reduced` once it does not.
pub fn reduce_operator(arena: &mut TypeArena, operator: Operator) -> Reduction {
    let before = take_cut_reasons();
    let result = match operator {
        Operator::Mapped(mapped) => reduce_mapped(arena, mapped),
        Operator::Keyof(operand) => reduce_keyof(arena, operand),
        Operator::IndexedAccess(object, index) => reduce_indexed_access(arena, object, index),
        Operator::StringMapping(case, operand) => reduce_string_mapping(arena, case, operand),
        Operator::Template(texts, types) => reduce_template(arena, texts, types),
        Operator::Variadic(elements) => reduce_variadic(arena, elements),
    };
    let during = take_cut_reasons();
    // Give back what was recorded before and during, so reporting is unchanged.
    for (reason, count) in before.iter().chain(during.iter()) {
        for _ in 0..*count {
            crate::stack_guard::note_cut(*reason);
        }
    }
    if !during.is_empty() {
        Reduction::TooComplex(result)
    } else if contains_type_param(arena, result) {
        Reduction::Deferred(result)
    } else {
        Reduction::Reduced(result)
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn an_operator_names_the_file_that_reduces_it() {
        let mut arena = TypeArena::new();
        let never = arena.never();
        assert_eq!(Operator::Keyof(never).name(), "keyof");
    }

    #[test]
    fn keyof_of_a_known_object_is_reduced_and_of_a_parameter_is_deferred() {
        let mut arena = TypeArena::new();
        let string = arena.string();
        let object = arena.alloc(Type::Object(ObjectType::new(vec![
            crate::types::PropertyEntry {
                name: "a".into(),
                type_id: string,
                optional: false,
                is_method: false,
                modifiers: Default::default(),
            },
        ])));
        assert!(matches!(
            reduce_operator(&mut arena, Operator::Keyof(object)),
            Reduction::Reduced(_)
        ));
        let parameter = arena.alloc(Type::GenericParameter(
            crate::types::TypeParameterId::new(0, 0),
            "T".into(),
            None,
        ));
        assert!(matches!(
            reduce_operator(&mut arena, Operator::Keyof(parameter)),
            Reduction::Deferred(_)
        ));
    }
}
