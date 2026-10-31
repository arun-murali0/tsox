use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::arena::{DeclSlot, TypeArena, TypeId};

// PartialEq, Eq and Hash on Type are written by hand (below the enum) rather than
// derived, for one reason: NumberLiteral(f64). f64 has no total Eq or Hash (NaN is
// not equal to itself), but TypeArena::alloc uses a Type as a hash-map key to
// reuse the TypeId of an identical composite. NumberLiteral is therefore compared
// and hashed by f64::to_bits(), so `==` and `hash` always agree with each other.
//
// Both are SHALLOW: a composite's TypeId fields are compared and hashed as opaque
// ids, never followed through the arena. That is what makes the intern table safe
// for self-referential types (two still-unresolved declarations have different Ref
// ids, so they never look alike) and it is only sound because every child id was itself
// obtained from the arena. A consequence: `==` on a Type still cannot answer "are
// these the same type by shape" for composites whose children are different ids.
// Use TypeArena::structurally_equal for that; it follows TypeIds through the arena.
#[derive(Clone, Debug)]
pub enum Type {
    Number,
    String,
    Boolean,
    Null,
    Undefined,

    Any,

    Unknown,

    Error,
    Function(FunctionType),
    Object(ObjectType),

    Array(TypeId),

    Union(Vec<TypeId>),

    StringLiteral(String),

    NumberLiteral(f64),
    BooleanLiteral(bool),

    Never,

    // A function with no meaningful return value (`function f(): void {}`).
    // Distinct from Undefined: undefined is assignable to void (an implicit or
    // bare `return;` satisfies a void return type), but void is not assignable
    // back to undefined or anything else concrete; see subtyping's own note on
    // this. Real TypeScript also lets a *function type* whose return is void
    // accept an implementation that returns something else, e.g. assigning
    // `() => number` where `() => void` is expected. That exception lives in
    // function signature compatibility, not in the primitive subtype relation.
    Void,

    // The Option is the parameter's own `extends` bound (`T extends { length:
    // number }`), resolved once when the parameter is first created and reused
    // from the same TypeParameterId cache as everything else about this
    // parameter. `None` means fully unconstrained.
    GenericParameter(TypeParameterId, String, Option<TypeId>),

    // The type of an interface, class or object-literal alias: a reference to the
    // declaration's slot in the arena. It is created before the declaration's
    // members are resolved and never changes afterwards, which is what lets a member
    // name the declaration back (`next: Node | null`) without anything being patched
    // when the body is finished. TypeArena::get looks through it to the body, so code
    // that reads a type never sees this variant; only the arena itself does.
    Ref(DeclSlot),

    // A type alias or an enum seen by its name: `type Scores = number[]` is Named(Scores,
    // the array) and prints as `Scores`. The second field is what it stands for, and
    // TypeArena::get looks through to it, so every relation treats the two as one type.
    // Wrapping is what lets the name live on the node and not on an id: the array is
    // the id every `number[]` shares, and nothing about it changes when it is named.
    Named(DeclSlot, TypeId),

    // A generic declaration applied to arguments: `Box<Dog>` is App(Box, [Dog], the
    // instantiated body). The body is what get() looks through to. The name is not
    // stored, it is written from the declaration and the arguments when the type is
    // displayed, so it is always the arguments' current spelling and nothing has to
    // check that a text baked in earlier is still right. Two applications with the same
    // declaration, arguments and body are the same id.
    App(DeclSlot, Vec<TypeId>, TypeId),

    // `A & B`. It sits last, as it was added. Variant position used to matter here: adding
    // a variant changed the derived hash, which changed which type digests collided, and
    // a colliding string literal was not interned, so literal narrowing gave never (the
    // four `Property 'payloadNN' does not exist on type 'never'` in the mixed benchmark).
    // TypeArena::alloc now keeps every colliding type under one id, so nothing depends on
    // the order of the variants any more.
    // The members are in the order they were written and are never sorted:
    // `A & B` and `B & A` are two ids that are assignable both ways, because the call
    // signatures of the members are tried in member order and a union's sorting would
    // lose that. Built only by TypeArena::alloc_intersection, which has already
    // flattened, distributed over unions, reduced primitives and dropped duplicates, so
    // there are at least two members and none of them is a union, an intersection,
    // `never`, `unknown` or `any`. LLD 1.13 has the rules and the reasons.
    Intersection(Vec<TypeId>),
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Type::Number, Type::Number)
            | (Type::String, Type::String)
            | (Type::Boolean, Type::Boolean)
            | (Type::Null, Type::Null)
            | (Type::Undefined, Type::Undefined)
            | (Type::Any, Type::Any)
            | (Type::Unknown, Type::Unknown)
            | (Type::Error, Type::Error)
            | (Type::Never, Type::Never)
            | (Type::Void, Type::Void) => true,

            (Type::Function(a), Type::Function(b)) => a == b,
            (Type::Object(a), Type::Object(b)) => a == b,
            (Type::Array(a), Type::Array(b)) => a == b,
            (Type::Union(a), Type::Union(b)) => a == b,
            (Type::Intersection(a), Type::Intersection(b)) => a == b,
            (Type::StringLiteral(a), Type::StringLiteral(b)) => a == b,
            // Bit pattern, not IEEE ==: keeps this consistent with Hash below, and
            // makes 0.0 and -0.0 distinct while a NaN literal equals itself.
            (Type::NumberLiteral(a), Type::NumberLiteral(b)) => a.to_bits() == b.to_bits(),
            (Type::BooleanLiteral(a), Type::BooleanLiteral(b)) => a == b,
            (
                Type::GenericParameter(id_a, name_a, bound_a),
                Type::GenericParameter(id_b, name_b, bound_b),
            ) => id_a == id_b && name_a == name_b && bound_a == bound_b,
            (Type::Ref(a), Type::Ref(b)) => a == b,
            (Type::Named(slot_a, inner_a), Type::Named(slot_b, inner_b)) => {
                slot_a == slot_b && inner_a == inner_b
            }
            (Type::App(slot_a, args_a, body_a), Type::App(slot_b, args_b, body_b)) => {
                slot_a == slot_b && args_a == args_b && body_a == body_b
            }

            // Different variants. Every variant is named here on purpose, with no
            // wildcard, so adding a Type variant is a compile error until it is
            // handled above instead of silently comparing unequal to itself.
            (
                Type::Number
                | Type::String
                | Type::Boolean
                | Type::Null
                | Type::Undefined
                | Type::Any
                | Type::Unknown
                | Type::Error
                | Type::Never
                | Type::Void
                | Type::Function(_)
                | Type::Object(_)
                | Type::Array(_)
                | Type::Union(_)
                | Type::Intersection(_)
                | Type::StringLiteral(_)
                | Type::NumberLiteral(_)
                | Type::BooleanLiteral(_)
                | Type::GenericParameter(..)
                | Type::Ref(_)
                | Type::Named(..)
                | Type::App(..),
                _,
            ) => false,
        }
    }
}

impl Eq for Type {}

impl Hash for Type {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Type::Number
            | Type::String
            | Type::Boolean
            | Type::Null
            | Type::Undefined
            | Type::Any
            | Type::Unknown
            | Type::Error
            | Type::Never
            | Type::Void => {}

            Type::Function(function) => function.hash(state),
            Type::Object(object) => object.hash(state),
            Type::Array(element) => element.hash(state),
            // Order-sensitive, matching the derived Vec equality above. Unions are
            // not keyed by Type in the arena's intern table (alloc_union interns
            // them by their finished member list instead), so this exists only to
            // keep Hash total.
            Type::Union(members) => members.hash(state),
            Type::Intersection(members) => members.hash(state),
            Type::StringLiteral(text) => text.hash(state),
            Type::NumberLiteral(value) => value.to_bits().hash(state),
            Type::BooleanLiteral(value) => value.hash(state),
            Type::GenericParameter(id, name, bound) => {
                id.hash(state);
                name.hash(state);
                bound.hash(state);
            }
            Type::Ref(slot) => slot.hash(state),
            Type::Named(slot, inner) => {
                slot.hash(state);
                inner.hash(state);
            }
            Type::App(slot, args, body) => {
                slot.hash(state);
                args.hash(state);
                body.hash(state);
            }
        }
    }
}

// Names one file of a project. It exists so that identity derived from a source
// position (below) stays unambiguous once several files share semantic state: byte
// offset 40 in `a.ts` and byte offset 40 in `b.ts` are different declarations. It is a
// plain index, not a path, so type identity never depends on a filesystem or an Oxc
// allocation; ProjectFiles owns the path-to-id mapping. Ordered so that project-level
// results (module graph layers, check reports) can be sorted into a stable order that
// does not depend on which thread finished first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FileId(u32);

impl FileId {
    /// The id every single-file check uses.
    pub const ROOT: Self = Self(0);

    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    pub const fn index(self) -> u32 {
        self.0
    }
}

// Identifies a declared type parameter by the file and position where it is written,
// not by an arena slot and not by an AST pointer. Two distinct resolutions of the same
// declared `T` (once while resolving a function's signature, again while checking its
// body) must agree on this value so both reuse the same `TypeId` for it. A
// source-derived key is stable across separate parses of unchanged source; a pointer
// or arena slot is only guaranteed stable within the one parse that produced it. The
// file component keeps two files' parameters apart when their offsets coincide.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TypeParameterId {
    file_id: FileId,
    declaration_span_start: u32,
    parameter_index: u32,
}

impl TypeParameterId {
    /// A parameter of the root file, for unit tests that build parameters directly.
    /// The checker itself always uses with_file, because it knows its file.
    #[cfg(test)]
    pub fn new(declaration_span_start: u32, parameter_index: u32) -> Self {
        Self::with_file(FileId::ROOT, declaration_span_start, parameter_index)
    }

    pub fn with_file(file_id: FileId, declaration_span_start: u32, parameter_index: u32) -> Self {
        Self {
            file_id,
            declaration_span_start,
            parameter_index,
        }
    }

    // The position of this parameter within its own declaration's type parameter
    // list (0 for the first `<T, ...>`, 1 for the second, and so on). Exposed so
    // an explicit call-site type argument list, which has no declaration_span of
    // its own to key off of, can still be zipped positionally against the
    // parameters found in a function's structural type; see
    // generics::ordered_generic_param_ids.
    pub fn parameter_index(&self) -> u32 {
        self.parameter_index
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionType {
    pub params: Vec<Param>,
    pub return_type: TypeId,
    /// The ordered type parameters declared by this signature itself. This is
    /// authoritative for explicit type arguments and defaults; discovering parameters
    /// only by walking input/return types loses unused parameters such as `<T>() => void`.
    pub type_params: Vec<FunctionTypeParam>,
    pub is_untyped: bool,
}

/// One type parameter owned by a function or method signature.
///
/// `constraint` and `default` are resolved TypeIds in the same arena as the signature.
/// The `id` is stable declaration identity, not an arena index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionTypeParam {
    pub id: TypeParameterId,
    pub name: String,
    pub constraint: Option<TypeId>,
    pub default: Option<TypeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Param {
    pub type_id: TypeId,
    pub optional: bool,

    pub rest: bool,

    // Kept for diagnostics only, naming a missing argument, so structural
    // equality ignores it; `(x: number) => void` and `(y: number) => void`
    // stay the same type.
    pub name: Option<Rc<str>>,
}

impl Param {
    #[allow(dead_code)]
    pub fn required(type_id: TypeId) -> Self {
        Self {
            type_id,
            optional: false,
            rest: false,
            name: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectType {
    // Always sorted by name. subtyping::object_is_subtype walks two of these in
    // step (a merge-join, not a lookup per property) and TypeArena::structurally_equal
    // compares them pairwise, and both are only correct on sorted input. Build one
    // with ObjectType::new, which sorts, and never assemble the struct by hand.
    //
    // A shared slice rather than a Vec: the walkers in semantic::generics, the call
    // checker and the class checker all take an owned copy of a Type out of the
    // arena (they need it after they start allocating), and with a Vec that copy
    // was a heap allocation plus one refcount bump per property, on every call
    // expression and every substitution step. Cloning the Rc is a single bump.
    // The list never changes after construction (a declaration is finished by pointing
    // its Ref at a body through TypeArena::resolve_ref), so sharing is safe. Hash
    // and equality see through the Rc, so the intern digests are unchanged.
    pub properties: Rc<[PropertyEntry]>,
}

impl ObjectType {
    // The only supported way to construct an ObjectType. Sorting an already sorted
    // list is a single linear pass, so callers that happen to have sorted input
    // pay almost nothing for not having to know that.
    pub fn new(mut properties: Vec<PropertyEntry>) -> Self {
        properties.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            properties: properties.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PropertyEntry {
    // `Rc<str>` rather than `String`: class inheritance resolution clones the whole
    // accumulated property list once per level of an inheritance chain (see
    // `resolve_class` in namespace.rs), so cloning a `PropertyEntry` is on the hot
    // path for any class hierarchy of meaningful depth. An `Rc<str>` clone is a
    // refcount bump; a `String` clone is a heap allocation and byte copy every time.
    pub name: Rc<str>,
    pub type_id: TypeId,
    pub optional: bool,

    // Declared with method syntax (`get(): T` in an interface, or a class method)
    // rather than as a function-typed property (`get: () => T`). tsc compares the
    // parameters of a method bivariantly even under strictFunctionTypes, and the
    // ones of a function-typed property contravariantly, and which rule applies
    // follows the *target* property's declaration (see subtyping::object_is_subtype).
    pub is_method: bool,
}

pub fn widen(arena: &TypeArena, type_id: TypeId) -> TypeId {
    match arena.get(type_id) {
        Type::StringLiteral(_) => arena.string(),
        Type::NumberLiteral(_) => arena.number(),
        Type::BooleanLiteral(_) => arena.boolean(),
        _ => type_id,
    }
}
