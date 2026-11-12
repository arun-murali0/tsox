use crate::diagnostic_codes::DiagnosticCode;

/// A diagnostic's code and text, produced together by exactly one
/// constructor per kind of diagnostic (see the `messages` module below).
///
/// This exists so a call site can never pass a `DiagnosticCode` and a
/// message string that don't actually correspond to each other; there is
/// no way to construct one of these except through a function that already
/// knows which code goes with which text. `ctx.error()`/`ctx.warning()`
/// take one of these instead of a `(code, message)` pair.
pub struct DiagnosticMessage {
    pub code: DiagnosticCode,
    pub text: String,
}

impl DiagnosticMessage {
    fn new(code: DiagnosticCode, text: impl Into<String>) -> Self {
        Self {
            code,
            text: text.into(),
        }
    }
}

/// One function per diagnostic kind. A `DiagnosticCode` that has more than
/// one distinct phrasing (e.g. "not callable" vs "not a constructor") gets
/// more than one function here, still sharing the one code; what this
/// module guarantees is that code and text are chosen together, in one
/// place, not that every code maps to exactly one string.
pub mod messages {
    use super::{DiagnosticCode, DiagnosticMessage};
    use crate::arena::{TypeArena, TypeId};
    use crate::explain::{
        Level, mismatch_children, mismatch_levels, mismatch_levels_via, render, top_code,
    };
    use crate::type_display::{display_source_type, display_type};
    use crate::types::Type;

    // A plain "not assignable" message, with the chain of reasons tsc puts under it. A
    // missing property that is the only reason replaces the first line, and with it the
    // code (TS2741 and its list forms), so the code is chosen here along with the text.
    fn not_assignable(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
        plain: DiagnosticCode,
    ) -> DiagnosticMessage {
        if matches!(
            plain,
            DiagnosticCode::DeclaredTypeMismatch | DiagnosticCode::ReturnTypeMismatch
        ) && is_readonly_to_mutable(arena, actual, expected)
        {
            return readonly_to_mutable(arena, actual, expected);
        }
        let levels = mismatch_levels(arena, actual, expected);
        DiagnosticMessage::new(top_code(&levels, plain), render(&levels))
    }

    // A message with a first line of its own (`Argument of type ...`) that keeps the
    // chain of reasons under it.
    fn headed(
        arena: &TypeArena,
        headline: String,
        actual: TypeId,
        expected: TypeId,
        code: DiagnosticCode,
    ) -> DiagnosticMessage {
        let mut levels = vec![Level::headline(headline)];
        levels.extend(mismatch_children(arena, actual, expected));
        DiagnosticMessage::new(code, render(&levels))
    }

    /// The code and text for `actual` not being assignable to `expected`, reported
    /// under `plain` when no missing property replaces it. Used where a caller reports
    /// a mismatch of its own kind (a property of an object literal, an arrow body).
    pub fn tuple_index_out_of_range(text: String) -> DiagnosticMessage {
        DiagnosticMessage::new(DiagnosticCode::TupleIndexOutOfRange, text)
    }

    pub fn class_incorrectly_extends(
        arena: &TypeArena,
        class_name: &str,
        base: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ClassIncorrectlyExtends,
            format!(
                "Class '{class_name}' incorrectly extends base class '{}'.",
                crate::type_display::display_type(arena, base)
            ),
        )
    }

    pub fn interface_incorrectly_extends(
        arena: &TypeArena,
        name: &str,
        base: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::InterfaceIncorrectlyExtends,
            format!(
                "Interface '{name}' incorrectly extends interface '{}'.",
                crate::type_display::display_type(arena, base)
            ),
        )
    }

    pub fn property_not_assignable_to_index(
        arena: &TypeArena,
        property: &str,
        property_type: TypeId,
        index_kind: &str,
        index_type: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PropertyNotAssignableToIndex,
            format!(
                "Property '{property}' of type '{}' is not assignable to '{index_kind}' index type '{}'.",
                crate::type_display::display_type(arena, property_type),
                crate::type_display::display_type(arena, index_type)
            ),
        )
    }

    pub fn missing_override_modifier(base_class: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::MissingOverrideModifier,
            format!(
                "This member must have an 'override' modifier because it overrides a member in the base class '{base_class}'."
            ),
        )
    }

    pub fn duplicate_index_signature(index_kind: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::DuplicateIndexSignature,
            format!("Duplicate index signature for type '{index_kind}'."),
        )
    }

    pub fn class_property_incompatible(
        arena: &TypeArena,
        property: &str,
        class_name: &str,
        interface: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ClassPropertyIncompatible,
            format!(
                "Property '{property}' in type '{class_name}' is not assignable to the same property in base type '{}'.",
                crate::type_display::display_type(arena, interface)
            ),
        )
    }

    pub fn class_incorrectly_implements(
        arena: &TypeArena,
        class_name: &str,
        interface: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ClassImplementsMismatch,
            format!(
                "Class '{class_name}' incorrectly implements interface '{}'.",
                crate::type_display::display_type(arena, interface)
            ),
        )
    }

    pub fn does_not_satisfy(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        use crate::type_display::{display_source_type, display_type};
        DiagnosticMessage::new(
            DiagnosticCode::SatisfiesMismatch,
            format!(
                "Type '{}' does not satisfy the expected type '{}'.",
                display_source_type(arena, actual, expected),
                display_type(arena, expected)
            ),
        )
    }

    pub fn assignability(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
        plain: DiagnosticCode,
    ) -> DiagnosticMessage {
        not_assignable(arena, actual, expected, plain)
    }

    pub fn binary_operand_type_mismatch(
        arena: &TypeArena,
        operator: &str,
        left: TypeId,
        right: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::BinaryOperandTypeMismatch,
            format!(
                "Operator '{operator}' cannot be applied to types '{}' and '{}'.",
                display_type(arena, left),
                display_type(arena, right)
            ),
        )
    }

    // tsc reports an arithmetic operator (`-`, `*`, ...) on a bad operand as one
    // diagnostic per bad side, not the "cannot be applied to types" of `+`.
    pub fn arithmetic_left_operand_invalid() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ArithmeticLeftOperandInvalid,
            "The left-hand side of an arithmetic operation must be of type 'any', 'number', 'bigint' or an enum type.",
        )
    }

    pub fn arithmetic_right_operand_invalid() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ArithmeticRightOperandInvalid,
            "The right-hand side of an arithmetic operation must be of type 'any', 'number', 'bigint' or an enum type.",
        )
    }

    pub fn argument_not_assignable(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        headed(
            arena,
            format!(
                "Argument of type '{}' is not assignable to parameter of type '{}'.",
                display_source_type(arena, actual, expected),
                display_type(arena, expected)
            ),
            actual,
            expected,
            DiagnosticCode::ArgumentNotAssignable,
        )
    }

    /// `declared_type_mismatch` for a generic source instantiated against the target:
    /// the first line names `actual` as declared, the reasons come from `instantiated`.
    pub fn declared_type_mismatch_via(
        arena: &TypeArena,
        actual: TypeId,
        instantiated: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        if is_readonly_to_mutable(arena, actual, expected) {
            return readonly_to_mutable(arena, actual, expected);
        }
        let levels = mismatch_levels_via(arena, actual, instantiated, expected);
        DiagnosticMessage::new(
            top_code(&levels, DiagnosticCode::DeclaredTypeMismatch),
            render(&levels),
        )
    }

    /// `argument_not_assignable` for a generic argument instantiated against its parameter.
    pub fn argument_not_assignable_via(
        arena: &TypeArena,
        actual: TypeId,
        instantiated: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        let mut levels = vec![Level::headline(format!(
            "Argument of type '{}' is not assignable to parameter of type '{}'.",
            display_source_type(arena, actual, expected),
            display_type(arena, expected)
        ))];
        levels.extend(mismatch_children(arena, instantiated, expected));
        DiagnosticMessage::new(DiagnosticCode::ArgumentNotAssignable, render(&levels))
    }

    pub fn return_type_mismatch(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        not_assignable(arena, actual, expected, DiagnosticCode::ReturnTypeMismatch)
    }

    pub fn declared_type_mismatch(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        not_assignable(
            arena,
            actual,
            expected,
            DiagnosticCode::DeclaredTypeMismatch,
        )
    }

    fn is_readonly_to_mutable(arena: &TypeArena, actual: TypeId, expected: TypeId) -> bool {
        matches!(arena.get(actual), Type::Array(_))
            && matches!(arena.get(expected), Type::Array(_))
            && arena.is_readonly_array(actual)
            && !arena.is_readonly_array(expected)
    }

    // `readonly T[]` into `T[]` is TS4104 on its own line, not TS2322.
    pub fn readonly_to_mutable(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ReadonlyToMutable,
            format!(
                "The type '{}' is 'readonly' and cannot be assigned to the mutable type '{}'.",
                display_type(arena, actual),
                display_type(arena, expected)
            ),
        )
    }

    pub fn readonly_index_write(arena: &TypeArena, object: TypeId) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ReadonlyIndexWrite,
            format!(
                "Index signature in type '{}' only permits reading.",
                display_type(arena, object)
            ),
        )
    }

    // A destructuring declaration whose initializer does not fit the annotation. tsc has no
    // wording of its own for this: it is the same TS2322 as any other assignment, with the
    // same chain of reasons under it, so the text is built the same way.
    pub fn destructuring_pattern_type_mismatch(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        not_assignable(
            arena,
            actual,
            expected,
            DiagnosticCode::DestructuringPatternTypeMismatch,
        )
    }

    pub fn static_field_initializer_mismatch(
        arena: &TypeArena,
        actual: TypeId,
        expected: TypeId,
    ) -> DiagnosticMessage {
        not_assignable(
            arena,
            actual,
            expected,
            DiagnosticCode::StaticFieldInitializerMismatch,
        )
    }

    // tsc's TS2344 names only the constraint, never the type parameter, so the message
    // carries no parameter name.
    pub fn type_argument_constraint_violation(
        arena: &TypeArena,
        actual: TypeId,
        constraint: TypeId,
    ) -> DiagnosticMessage {
        headed(
            arena,
            format!(
                "Type '{}' does not satisfy the constraint '{}'.",
                display_type(arena, actual),
                display_type(arena, constraint)
            ),
            actual,
            constraint,
            DiagnosticCode::TypeArgumentConstraintViolation,
        )
    }

    pub fn excess_property(arena: &TypeArena, name: &str, target: TypeId) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ExcessProperty,
            format!(
                "Object literal may only specify known properties, and '{name}' does not exist in type '{}'.",
                display_type(arena, target)
            ),
        )
    }

    pub fn property_not_initialized(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PropertyNotInitialized,
            format!(
                "Property '{name}' has no initializer and is not definitely assigned in the constructor."
            ),
        )
    }

    pub fn parameter_implicitly_any(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyParameter,
            format!("Parameter '{name}' implicitly has an 'any' type."),
        )
    }

    // tsc's own code (TS7019) and wording for a rest parameter with no type: a
    // separate diagnostic from the plain-parameter one (TS7006).
    pub fn rest_parameter_implicitly_any(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyRestParameter,
            format!("Rest parameter '{name}' implicitly has an 'any[]' type."),
        )
    }

    pub fn element_implicitly_any(
        arena: &TypeArena,
        key: TypeId,
        object: TypeId,
    ) -> DiagnosticMessage {
        let mut text = format!(
            "Element implicitly has an 'any' type because expression of type '{}' can't be used to index type '{}'.",
            display_type(arena, key),
            display_type(arena, object)
        );
        // tsc adds why when the key is a whole primitive: the object has no index
        // signature of that kind. A literal key gets a different message altogether.
        if matches!(
            arena.get(key),
            crate::types::Type::String | crate::types::Type::Number
        ) {
            text.push_str(&format!(
                "\n  No index signature with a parameter of type '{}' was found on type '{}'.",
                display_type(arena, key),
                display_type(arena, object)
            ));
        }
        if let crate::types::Type::StringLiteral(name) = arena.get(key) {
            text.push_str(&format!(
                "\n  Property '{}' does not exist on type '{}'.",
                name,
                display_type(arena, object)
            ));
        }
        DiagnosticMessage::new(DiagnosticCode::ImplicitAnyElement, text)
    }

    // `t[5]` on `[string, number]`: tsc names the tuple, its length and the index.
    pub fn tuple_index_out_of_bounds(
        arena: &TypeArena,
        tuple: TypeId,
        length: usize,
        index: usize,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TupleIndexOutOfBounds,
            format!(
                "Tuple type '{}' of length '{length}' has no element at index '{index}'.",
                display_type(arena, tuple)
            ),
        )
    }

    // The three tuple shape rules tsc reports as grammar errors, in tsc's words.
    pub fn tuple_required_after_optional() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TupleRequiredAfterOptional,
            "A required element cannot follow an optional element.",
        )
    }

    pub fn tuple_rest_after_rest() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TupleRestAfterRest,
            "A rest element cannot follow another rest element.",
        )
    }

    pub fn tuple_optional_after_rest() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TupleOptionalAfterRest,
            "An optional element cannot follow a rest element.",
        )
    }

    pub fn tuple_negative_index() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TupleNegativeIndex,
            "A tuple type cannot be indexed with a negative value.",
        )
    }

    pub fn return_implicitly_any(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyReturn,
            format!(
                "'{name}' implicitly has return type 'any' because it does not have a return type annotation and is referenced directly or indirectly in one of its return expressions."
            ),
        )
    }

    pub fn setter_property_implicitly_any(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnySetter,
            format!(
                "Property '{name}' implicitly has type 'any', because its set accessor lacks a parameter type annotation."
            ),
        )
    }

    pub fn member_implicitly_any(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyMember,
            format!("Member '{name}' implicitly has an 'any' type."),
        )
    }

    pub fn function_lacks_return_annotation(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyFunctionReturn,
            format!(
                "'{name}', which lacks return-type annotation, implicitly has an 'any' return type."
            ),
        )
    }

    pub fn call_signature_lacks_return_annotation() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyCallSignature,
            "Call signature, which lacks return-type annotation, implicitly has an 'any' return type.",
        )
    }

    pub fn construct_signature_lacks_return_annotation() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyConstructSignature,
            "Construct signature, which lacks return-type annotation, implicitly has an 'any' return type.",
        )
    }

    pub fn this_implicitly_any() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyThis,
            "'this' implicitly has type 'any' because it does not have a type annotation.",
        )
    }

    // tsc says the same thing for every callee, with the reason under it: a primitive is named
    // by its wrapper (`Type 'Number' has no call signatures.`, also for a literal), a union by
    // the whole union, anything else by its own type.
    fn no_signatures_of(arena: &TypeArena, callee: TypeId) -> String {
        match arena.get(callee) {
            Type::Number | Type::NumberLiteral(_) => "Number".to_string(),
            Type::String | Type::StringLiteral(_) => "String".to_string(),
            Type::Boolean | Type::BooleanLiteral(_) => "Boolean".to_string(),
            _ => display_type(arena, callee),
        }
    }

    pub fn not_callable(arena: &TypeArena, callee: TypeId) -> DiagnosticMessage {
        let reason = if matches!(arena.get(callee), Type::Union(_)) {
            format!(
                "No constituent of type '{}' is callable.",
                display_type(arena, callee)
            )
        } else {
            format!(
                "Type '{}' has no call signatures.",
                no_signatures_of(arena, callee)
            )
        };
        DiagnosticMessage::new(
            DiagnosticCode::NotCallable,
            format!("This expression is not callable.\n  {reason}"),
        )
    }

    pub fn not_a_constructor(arena: &TypeArena, callee: TypeId) -> DiagnosticMessage {
        let reason = if matches!(arena.get(callee), Type::Union(_)) {
            format!(
                "No constituent of type '{}' is constructable.",
                display_type(arena, callee)
            )
        } else {
            format!(
                "Type '{}' has no construct signatures.",
                no_signatures_of(arena, callee)
            )
        };
        DiagnosticMessage::new(
            DiagnosticCode::NotConstructor,
            format!("This expression is not constructable.\n  {reason}"),
        )
    }

    // tsc's TS2554 always says "arguments", even for one, and never names the
    // missing parameter, so these match it word for word.
    pub fn argument_arity_exact(required: usize, got: usize) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ArgumentArityMismatch,
            format!("Expected {required} arguments, but got {got}."),
        )
    }

    // `resolve()` of a `Promise<T>` whose `T` is not `void`.
    pub fn promise_resolve_needs_value(got: usize) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PromiseResolveNeedsValue,
            format!(
                "Expected 1 arguments, but got {got}. Did you forget to include 'void' in your type argument to 'Promise'?"
            ),
        )
    }

    pub fn argument_arity_at_least(required: usize, got: usize) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ArgumentArityAtLeast,
            format!("Expected at least {required} arguments, but got {got}."),
        )
    }

    pub fn argument_arity_range(required: usize, max: usize, got: usize) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ArgumentArityMismatch,
            format!("Expected {required}-{max} arguments, but got {got}."),
        )
    }

    // tsc's TS2556: reported on the spread itself, in place of any arity message, when an
    // array (not a tuple) is spread where it cannot be matched to a rest parameter.
    pub fn spread_argument_needs_tuple_or_rest() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::SpreadArgumentNotTupleOrRest,
            "A spread argument must either have a tuple type or be passed to a rest parameter.",
        )
    }

    // tsc's TS2769. `chain` is the whole text, headline and the lines under it, built by the
    // call checker: unlike a mismatch, it has one branch per overload, which the linear
    // `Level` chain cannot describe.
    pub fn no_overload_matches(chain: String) -> DiagnosticMessage {
        DiagnosticMessage::new(DiagnosticCode::NoOverloadMatches, chain)
    }

    // tsc's TS2590: distributing an intersection over unions would make more members than
    // it is willing to build.
    pub fn expression_too_complex() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ExpressionTooComplex,
            "Expression produces a union type that is too complex to represent.",
        )
    }

    // Not a tsc diagnostic: the checker stopped on an internal error while checking the
    // file, so the diagnostics for it are missing or incomplete.
    pub fn internal_checker_error() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::InternalCheckerError,
            "The checker stopped on an internal error; the results for this file are incomplete.",
        )
    }

    // Not a tsc diagnostic: the file nests expressions or declarations deeper than the
    // checker's stack allows, so it stopped descending there and the types below that
    // point are unknown. tsc has no limit of this kind; it would overflow its own stack.
    pub fn nesting_too_deep() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::NestingTooDeep,
            "This code is nested too deeply for the checker; the types below this point are not checked.",
        )
    }

    pub fn module_not_found(specifier: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ModuleNotFound,
            format!("Cannot find module '{specifier}' or its corresponding type declarations."),
        )
    }

    pub fn type_definition_not_found(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TypeDefinitionNotFound,
            format!("Cannot find type definition file for '{name}'."),
        )
    }

    pub fn referenced_file_not_found(path: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ReferencedFileNotFound,
            format!("File '{path}' not found."),
        )
    }

    pub fn no_exported_member(specifier: &str, name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::NoExportedMember,
            format!("Module '\"{specifier}\"' has no exported member '{name}'."),
        )
    }

    pub fn no_default_export(specifier: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::NoDefaultExport,
            format!("Module '\"{specifier}\"' has no default export."),
        )
    }

    // A name that is a type and not a value, written where a value is expected.
    pub fn type_used_as_value(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TypeUsedAsValue,
            format!("'{name}' only refers to a type, but is being used as a value here."),
        )
    }

    pub fn import_type_only_value(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImportTypeOnlyValue,
            format!(
                "'{name}' cannot be used as a value because it was imported using 'import type'."
            ),
        )
    }

    // tsc points a few well-known global names at the package that declares them.
    pub fn unresolved_identifier_hint(name: &str) -> Option<DiagnosticMessage> {
        const NODE: &[&str] = &[
            "require",
            "module",
            "exports",
            "process",
            "Buffer",
            "__filename",
            "__dirname",
            "fs",
            "path",
            "url",
            "events",
            "http",
            "https",
            "os",
            "util",
            "crypto",
            "stream",
            "child_process",
            "assert",
            "zlib",
            "net",
            "readline",
            "cluster",
            "dns",
            "vm",
            "querystring",
            "buffer",
            "tty",
            "worker_threads",
        ];
        let node_scheme = name.starts_with("node:");
        if NODE.contains(&name) || node_scheme {
            return Some(DiagnosticMessage::new(
                DiagnosticCode::MissingNodeTypes,
                format!(
                    "Cannot find name '{name}'. Do you need to install type definitions for node? Try `npm i --save-dev @types/node` and then add 'node' to the types field in your tsconfig."
                ),
            ));
        }
        match name {
            "describe" | "suite" | "it" | "test" => Some(DiagnosticMessage::new(
                DiagnosticCode::MissingTestRunnerTypes,
                format!(
                    "Cannot find name '{name}'. Do you need to install type definitions for a test runner? Try `npm i --save-dev @types/jest` or `npm i --save-dev @types/mocha` and then add 'jest' or 'mocha' to the types field in your tsconfig."
                ),
            )),
            "$" | "jQuery" => Some(DiagnosticMessage::new(
                DiagnosticCode::MissingJqueryTypes,
                format!(
                    "Cannot find name '{name}'. Do you need to install type definitions for jQuery? Try `npm i --save-dev @types/jquery` and then add 'jquery' to the types field in your tsconfig."
                ),
            )),
            _ => None,
        }
    }

    pub fn variable_implicitly_any(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyVariable,
            format!("Variable '{name}' implicitly has an 'any' type."),
        )
    }

    pub fn binding_element_implicitly_any(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImplicitAnyBinding,
            format!("Binding element '{name}' implicitly has an 'any' type."),
        )
    }

    pub fn subsequent_variable_type(
        name: &str,
        previous: &str,
        current: &str,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::SubsequentVariableType,
            format!(
                "Subsequent variable declarations must have the same type.  Variable '{name}' must be of type '{previous}', but here has type '{current}'."
            ),
        )
    }

    pub fn unresolved_namespace(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnresolvedNamespace,
            format!("Cannot find namespace '{name}'."),
        )
    }

    pub fn unresolved_identifier(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnresolvedIdentifier,
            format!("Cannot find name '{name}'."),
        )
    }

    pub fn property_does_not_exist(
        arena: &TypeArena,
        property_name: &str,
        object_type: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PropertyDoesNotExist,
            format!(
                "Property '{property_name}' does not exist on type '{}'.",
                display_type(arena, object_type)
            ),
        )
    }

    pub fn property_does_not_exist_suggestion(
        arena: &TypeArena,
        property_name: &str,
        object_type: TypeId,
        suggestion: &str,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PropertyDoesNotExistSuggestion,
            format!(
                "Property '{property_name}' does not exist on type '{}'. Did you mean '{suggestion}'?",
                display_type(arena, object_type)
            ),
        )
    }

    pub fn property_from_index_signature(property_name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PropertyFromIndexSignature,
            format!(
                "Property '{property_name}' comes from an index signature, so it must be accessed with ['{property_name}']."
            ),
        )
    }

    pub fn conversion_may_be_mistake(
        arena: &TypeArena,
        source: TypeId,
        target: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ConversionMayBeMistake,
            format!(
                "Conversion of type '{}' to type '{}' may be a mistake because neither type sufficiently overlaps with the other. If this was intentional, convert the expression to 'unknown' first.",
                display_type(arena, source),
                display_type(arena, target)
            ),
        )
    }

    pub fn comparison_no_overlap(
        arena: &TypeArena,
        left: TypeId,
        right: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ComparisonNoOverlap,
            format!(
                "This comparison appears to be unintentional because the types '{}' and '{}' have no overlap.",
                display_type(arena, left),
                display_type(arena, right)
            ),
        )
    }

    pub fn array_destructuring_requires_array(
        arena: &TypeArena,
        source: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ArrayDestructuringRequiresArray,
            format!(
                "Type '{}' must have a '[Symbol.iterator]()' method that returns an iterator.",
                display_type(arena, source)
            ),
        )
    }

    pub fn unimplemented_call_expression_kind() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedCallExpressionKind,
            "This kind of call expression is not yet checked by tsox.",
        )
    }

    pub fn unimplemented_new_expression_target() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedNewExpressionTarget,
            "`new` on anything other than a plain name is not yet checked by tsox.",
        )
    }

    pub fn untyped_parameter_skips_arity_check(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UntypedParameterSkipsArityCheck,
            format!(
                "'{name}' has an untyped parameter, so tsox can't check this call's arity yet."
            ),
        )
    }

    pub fn untyped_constructor_parameter_skips_arity_check(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UntypedParameterSkipsArityCheck,
            format!(
                "'{name}' has a constructor with an untyped parameter, so tsox can't \
                 check arity for `new {name}(...)` yet."
            ),
        )
    }

    pub fn unimplemented_computed_member_key() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedComputedMemberKey,
            "Computed member access with a non-literal key is not yet checked by tsox.",
        )
    }

    pub fn unimplemented_optional_chain_link() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedOptionalChainLink,
            "This kind of optional-chain link is not yet checked by tsox.",
        )
    }

    pub fn unimplemented_expression_kind() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedExpressionKind,
            "This expression kind is not yet checked by tsox.",
        )
    }

    pub fn unimplemented_static_accessor() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedStaticAccessor,
            "Static getter/setter is not yet checked by tsox.",
        )
    }

    pub fn unimplemented_class_shape(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedClassShape,
            format!(
                "Class '{name}' uses a shape not yet checked by tsox: an unresolvable \
                 field or method, or a superclass that isn't a plain class name."
            ),
        )
    }

    pub fn unimplemented_destructuring_key() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedDestructuringKey,
            "Computed or non-identifier destructuring keys are not yet checked by tsox.",
        )
    }

    pub fn unimplemented_rest_destructuring() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedRestDestructuring,
            "Rest destructuring (`...rest`) does not yet compute a precise type; \
             the binding is not checked by tsox.",
        )
    }

    pub fn unimplemented_statement_kind(kind: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnimplementedStatementKind,
            format!("This statement kind is not yet checked by tsox: {kind}."),
        )
    }

    pub fn unresolvable_type_parameter_constraint(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnresolvableTypeParameterConstraint,
            format!(
                "The constraint of type parameter '{name}' could not be resolved, so it is \
                 treated as unconstrained: it is not enforced at call sites and its members \
                 are not usable inside the function."
            ),
        )
    }

    // `required == total` is tsc's TS2314; a range (some trailing parameters have
    // defaults) is its TS2707. `display_name` is the generic's name with its
    // parameter list (`Box<T>`), because tsc prints it that way.
    pub fn type_argument_count_mismatch(
        display_name: &str,
        required: usize,
        total: usize,
    ) -> DiagnosticMessage {
        let text = if required == total {
            format!("Generic type '{display_name}' requires {total} type argument(s).")
        } else {
            format!(
                "Generic type '{display_name}' requires between {required} and {total} type arguments."
            )
        };
        let code = if required == total {
            DiagnosticCode::TypeArgumentCountMismatch
        } else {
            DiagnosticCode::TypeArgumentCountRange
        };
        DiagnosticMessage::new(code, text)
    }

    // TS2558 applies to an explicit type-argument list on a call or constructor.
    // Unlike generic type references, its message is framed around the count written.
    // tsc names a range when defaults make some of the parameters optional
    // ("Expected 1-2 type arguments, but got 3.") and one number when there is no choice.
    pub fn call_type_argument_count_mismatch(
        min: usize,
        max: usize,
        actual: usize,
    ) -> DiagnosticMessage {
        let expected = if min == max {
            min.to_string()
        } else {
            format!("{min}-{max}")
        };
        DiagnosticMessage::new(
            DiagnosticCode::TypeArgumentListCountMismatch,
            format!("Expected {expected} type arguments, but got {actual}."),
        )
    }

    pub fn duplicate_block_scoped_variable(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::DuplicateBlockScopedVariable,
            format!("Cannot redeclare block-scoped variable '{name}'."),
        )
    }

    pub fn duplicate_type_declaration(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::DuplicateTypeDeclaration,
            format!("Duplicate identifier '{name}'."),
        )
    }

    pub fn namespace_export_mismatch(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::NamespaceExportMismatch,
            format!(
                "Individual declarations in merged declaration '{name}' must be all exported or all local."
            ),
        )
    }

    pub fn namespace_merge_order() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::NamespaceMergeOrder,
            "A namespace declaration cannot be located prior to a class or function with which it is merged.",
        )
    }

    pub fn property_declarations_must_have_identical_modifiers(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PropertyDeclarationsMustHaveIdenticalModifiers,
            format!("All declarations of '{name}' must have identical modifiers."),
        )
    }

    pub fn global_augmentation_outside_module() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::GlobalAugmentationOutsideModule,
            "Augmentations for the global scope can only be directly nested in external modules or ambient module declarations.".to_string(),
        )
    }

    pub fn invalid_module_name_in_augmentation(specifier: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::InvalidModuleNameInAugmentation,
            format!("Invalid module name in augmentation, module '{specifier}' cannot be found."),
        )
    }

    pub fn ambient_module_relative_name() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AmbientModuleRelativeName,
            "Ambient module declaration cannot specify relative module name.".to_string(),
        )
    }

    pub fn ambient_initializer() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AmbientInitializer,
            "Initializers are not allowed in ambient contexts.".to_string(),
        )
    }

    pub fn ambient_implementation() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AmbientImplementation,
            "An implementation cannot be declared in ambient contexts.".to_string(),
        )
    }

    pub fn assignment_to_constant(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AssignmentToConstant,
            format!("Cannot assign to '{name}' because it is a constant."),
        )
    }

    pub fn assignment_to_class(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AssignmentToClass,
            format!("Cannot assign to '{name}' because it is a class."),
        )
    }

    pub fn assignment_to_function(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AssignmentToFunction,
            format!("Cannot assign to '{name}' because it is a function."),
        )
    }

    pub fn assignment_to_enum(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AssignmentToEnum,
            format!("Cannot assign to '{name}' because it is an enum."),
        )
    }

    // tsc's TS18050, for a literal `null` or `undefined` used as an operand.
    pub fn value_cannot_be_used_here(value: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ValueCannotBeUsedHere,
            format!("The value '{value}' cannot be used here."),
        )
    }

    pub fn assignment_to_import(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AssignmentToImport,
            format!("Cannot assign to '{name}' because it is an import."),
        )
    }

    pub fn interface_type_parameters_mismatch(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::InterfaceTypeParametersMismatch,
            format!("All declarations of '{name}' must have identical type parameters."),
        )
    }

    pub fn private_property_access(name: &str, class: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::PrivatePropertyAccess,
            format!("Property '{name}' is private and only accessible within class '{class}'."),
        )
    }

    pub fn protected_property_access(name: &str, class: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ProtectedPropertyAccess,
            format!(
                "Property '{name}' is protected and only accessible within class '{class}' and its subclasses."
            ),
        )
    }

    pub fn readonly_property_assignment(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ReadonlyPropertyAssignment,
            format!("Cannot assign to '{name}' because it is a read-only property."),
        )
    }

    pub fn import_assignment_in_es_modules() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ImportAssignmentInEsModules,
            "Import assignment cannot be used when targeting ECMAScript modules. Consider using 'import * as ns from \"mod\"', 'import {a} from \"mod\"', 'import d from \"mod\"', or another module format instead.".to_string(),
        )
    }

    pub fn export_assignment_in_es_modules() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ExportAssignmentInEsModules,
            "Export assignment cannot be used when targeting ECMAScript modules. Consider using 'export default' or another module format instead.".to_string(),
        )
    }

    pub fn export_star_ambiguity(specifier: &str, name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ExportStarAmbiguity,
            format!(
                "Module '{specifier}' has already exported a member named '{name}'. Consider explicitly re-exporting to resolve the ambiguity."
            ),
        )
    }

    pub fn duplicate_function_implementation() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::DuplicateFunctionImplementation,
            "Duplicate function implementation.".to_string(),
        )
    }

    pub fn subsequent_property_declaration_must_match(
        arena: &TypeArena,
        name: &str,
        existing: TypeId,
        incoming: TypeId,
    ) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::SubsequentPropertyDeclarationMustMatch,
            format!(
                "Subsequent property declarations must have the same type.  Property '{name}' must be of type '{}', but here has type '{}'.",
                display_type(arena, existing),
                display_type(arena, incoming),
            ),
        )
    }

    // An operand that may be null or undefined where a number is needed. tsc names the
    // operand when it is a plain name or a chain of property accesses (`'o.v' is
    // possibly 'undefined'.`, TS18047-9) and says "Object" for any other expression
    // (TS2531-3). `name` is that text, or `None` for the second case.
    pub fn possibly_nullish(name: Option<&str>, null: bool, undefined: bool) -> DiagnosticMessage {
        let what = match (null, undefined) {
            (true, true) => "'null' or 'undefined'",
            (true, false) => "'null'",
            _ => "'undefined'",
        };
        let (code, subject) = match (name, null, undefined) {
            (Some(name), true, true) => {
                (DiagnosticCode::PossiblyNullOrUndefined, format!("'{name}'"))
            }
            (Some(name), true, false) => (DiagnosticCode::PossiblyNull, format!("'{name}'")),
            (Some(name), _, _) => (DiagnosticCode::PossiblyUndefined, format!("'{name}'")),
            (None, true, true) => (
                DiagnosticCode::ObjectPossiblyNullOrUndefined,
                "Object".to_string(),
            ),
            (None, true, false) => (DiagnosticCode::ObjectPossiblyNull, "Object".to_string()),
            (None, _, _) => (
                DiagnosticCode::ObjectPossiblyUndefined,
                "Object".to_string(),
            ),
        };
        DiagnosticMessage::new(code, format!("{subject} is possibly {what}."))
    }

    // tsc's TS2567: an enum redeclared as anything but an enum or a namespace is not a
    // plain duplicate identifier, it has a message of its own.
    pub fn enum_declaration_merge() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::EnumDeclarationMerge,
            "Enum declarations can only merge with namespace or other enum declarations.",
        )
    }

    pub fn enum_multiple_declarations_require_initializer() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::EnumMultipleDeclarationsRequireInitializer,
            "In an enum with multiple declarations, only one declaration can omit an initializer for its first enum element.",
        )
    }

    pub fn type_is_not_generic(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::TypeIsNotGeneric,
            format!("Type '{name}' is not generic."),
        )
    }

    pub fn unresolvable_type_annotation(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnresolvableTypeAnnotation,
            format!(
                "Type annotation for '{name}' could not be resolved (unknown name, \
                 or its definition isn't fully understood by tsox yet)."
            ),
        )
    }

    pub fn unreachable_code() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnreachableCode,
            "Unreachable code detected.",
        )
    }

    pub fn fallthrough_case_in_switch() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::FallthroughCaseInSwitch,
            "Fallthrough case in switch.",
        )
    }

    pub fn flow_shadow_disagreement(name: &str, overlay: &str, flow: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::FlowShadowDisagreement,
            format!(
                "flow shadow: '{name}' is '{overlay}' by the forward pass and '{flow}' by the flow walk."
            ),
        )
    }

    pub fn flow_shadow_read(name: &str, ty: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::FlowShadowRead,
            format!("flow shadow: '{name}' is '{ty}' by both."),
        )
    }

    pub fn not_all_code_paths_return() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::NotAllCodePathsReturn,
            "Not all code paths return a value.",
        )
    }

    pub fn lacks_ending_return() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::LacksEndingReturn,
            "Function lacks ending return statement and return type does not include 'undefined'.",
        )
    }

    pub fn must_return_a_value() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::MustReturnValue,
            "A function whose declared type is neither 'undefined', 'void', nor 'any' must return a value.",
        )
    }

    pub fn never_has_reachable_end() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::NeverHasReachableEnd,
            "A function returning 'never' cannot have a reachable end point.",
        )
    }

    pub fn used_before_assigned(name: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UsedBeforeAssigned,
            format!("Variable '{name}' is used before being assigned."),
        )
    }

    pub fn unresolvable_destructuring_type_annotation() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::UnresolvableDestructuringTypeAnnotation,
            "Type annotation for this destructuring pattern could not be resolved.",
        )
    }

    // The expression-grammar diagnostics below carry tsc's exact wording (tsc 6.0.3),
    // because the checker's own code here is a direct port of the syntactic rules the
    // compiler applies, and the ts-diag-tool compare reads the text back.
    pub fn comma_left_no_side_effect() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::CommaLeftNoSideEffect,
            "Left side of comma operator is unused and has no side effects.",
        )
    }

    pub fn void_truthiness() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::VoidTruthiness,
            "An expression of type 'void' cannot be tested for truthiness.",
        )
    }

    pub fn always_truthy_expression() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AlwaysTruthyExpression,
            "This kind of expression is always truthy.",
        )
    }

    pub fn always_falsy_expression() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::AlwaysFalsyExpression,
            "This kind of expression is always falsy.",
        )
    }

    pub fn delete_operand_not_property() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::DeleteOperandNotProperty,
            "The operand of a 'delete' operator must be a property reference.",
        )
    }

    pub fn delete_operand_required() -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::DeleteOperandRequired,
            "The operand of a 'delete' operator must be optional.",
        )
    }

    pub fn excessive_complexity(source: &str, target: &str) -> DiagnosticMessage {
        DiagnosticMessage::new(
            DiagnosticCode::ExcessiveComplexity,
            format!("Excessive complexity comparing types '{source}' and '{target}'."),
        )
    }
}
