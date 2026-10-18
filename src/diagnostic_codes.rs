/// Stable identifiers for every diagnostic ts-rust can currently emit.
///
/// This is ts-rust's own namespace ("TSR####"), not TypeScript's TS####
/// numbering: ts-rust never emits tsc's codes as its own. What it does keep is a
/// one-to-one map to them. Every variant here stands for exactly one tsc
/// diagnostic (`tsc_code`), which is why a condition tsc reports under several codes
/// (a missing property is TS2741, a missing list TS2739, ...) has several variants
/// here and not one that is "mostly" right. A variant with no tsc equivalent (the
/// 9000s, which are "not yet checked" markers) maps to `None`.
///
/// The map is what lets scripts/ts-diag-tool/compare.js compare code and message
/// text, and not only the line a diagnostic is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "wasm", derive(serde::Serialize, serde::Deserialize))]
pub enum DiagnosticCode {
    // 1000s: assignability / type mismatch
    BinaryOperandTypeMismatch,
    ArgumentNotAssignable,
    ReturnTypeMismatch,
    DeclaredTypeMismatch,
    DestructuringPatternTypeMismatch,
    StaticFieldInitializerMismatch,
    TypeArgumentConstraintViolation,
    ExcessProperty,
    PropertyNotInitialized,
    MissingProperty,
    MissingProperties,
    MissingPropertiesMany,
    ArithmeticLeftOperandInvalid,
    ArithmeticRightOperandInvalid,
    PossiblyNull,
    PossiblyUndefined,
    PossiblyNullOrUndefined,
    ObjectPossiblyNull,
    ObjectPossiblyUndefined,
    ObjectPossiblyNullOrUndefined,

    // 1100s: calls
    NotCallable,
    NotConstructor,
    ArgumentArityMismatch,
    ArgumentArityAtLeast,
    SpreadArgumentNotTupleOrRest,
    NoOverloadMatches,

    // 1200s: name / member resolution
    UnresolvedIdentifier,
    PropertyDoesNotExist,
    ModuleNotFound,
    NoExportedMember,
    NoDefaultExport,
    TypeUsedAsValue,

    // 1300s: destructuring
    ArrayDestructuringRequiresArray,

    // 1500s: generics
    TypeArgumentCountMismatch,
    TypeArgumentCountRange,
    TypeIsNotGeneric,
    DuplicateTypeDeclaration,
    EnumDeclarationMerge,

    // 1400s: implicit any
    ImplicitAnyParameter,
    ImplicitAnyElement,
    ImplicitAnyThis,

    // 1600s: reachability
    UnreachableCode,

    // 1700s: type construction
    ExpressionTooComplex,

    // 9000s: not yet implemented (all currently emitted as warnings)
    UnimplementedCallExpressionKind,
    UnimplementedNewExpressionTarget,
    UntypedParameterSkipsArityCheck,
    UnimplementedComputedMemberKey,
    UnimplementedOptionalChainLink,
    UnimplementedExpressionKind,
    UnimplementedStaticAccessor,
    UnimplementedClassShape,
    UnimplementedDestructuringKey,
    UnimplementedRestDestructuring,
    UnimplementedStatementKind,
    UnresolvableTypeAnnotation,
    UnresolvableDestructuringTypeAnnotation,
    UnresolvableTypeParameterConstraint,

    // 9100s: the checker itself failed (no tsc counterpart)
    InternalCheckerError,
    NestingTooDeep,
}

impl DiagnosticCode {
    pub fn as_str(self) -> &'static str {
        use DiagnosticCode::*;
        match self {
            BinaryOperandTypeMismatch => "TSR1001",
            ArgumentNotAssignable => "TSR1002",
            ReturnTypeMismatch => "TSR1003",
            DeclaredTypeMismatch => "TSR1004",
            DestructuringPatternTypeMismatch => "TSR1005",
            StaticFieldInitializerMismatch => "TSR1006",
            TypeArgumentConstraintViolation => "TSR1007",
            ExcessProperty => "TSR1008",
            PropertyNotInitialized => "TSR1009",
            MissingProperty => "TSR1010",
            MissingProperties => "TSR1011",
            MissingPropertiesMany => "TSR1012",
            ArithmeticLeftOperandInvalid => "TSR1013",
            ArithmeticRightOperandInvalid => "TSR1014",
            PossiblyNull => "TSR1015",
            PossiblyUndefined => "TSR1016",
            PossiblyNullOrUndefined => "TSR1017",
            ObjectPossiblyNull => "TSR1018",
            ObjectPossiblyUndefined => "TSR1019",
            ObjectPossiblyNullOrUndefined => "TSR1020",

            NotCallable => "TSR1101",
            ArgumentArityMismatch => "TSR1102",
            NotConstructor => "TSR1103",
            ArgumentArityAtLeast => "TSR1104",
            SpreadArgumentNotTupleOrRest => "TSR1105",
            NoOverloadMatches => "TSR1106",

            UnresolvedIdentifier => "TSR1201",
            PropertyDoesNotExist => "TSR1202",
            ModuleNotFound => "TSR1203",
            NoExportedMember => "TSR1204",
            NoDefaultExport => "TSR1205",
            TypeUsedAsValue => "TSR1206",

            ArrayDestructuringRequiresArray => "TSR1301",

            TypeArgumentCountMismatch => "TSR1501",
            TypeIsNotGeneric => "TSR1502",
            DuplicateTypeDeclaration => "TSR1503",
            TypeArgumentCountRange => "TSR1504",
            EnumDeclarationMerge => "TSR1505",

            ImplicitAnyParameter => "TSR1401",
            ImplicitAnyElement => "TSR1402",
            ImplicitAnyThis => "TSR1403",

            UnimplementedCallExpressionKind => "TSR9001",
            UnimplementedNewExpressionTarget => "TSR9002",
            UntypedParameterSkipsArityCheck => "TSR9003",
            UnimplementedComputedMemberKey => "TSR9004",
            UnimplementedOptionalChainLink => "TSR9005",
            UnimplementedExpressionKind => "TSR9006",
            UnimplementedStaticAccessor => "TSR9007",
            UnimplementedClassShape => "TSR9008",
            UnimplementedDestructuringKey => "TSR9009",
            UnimplementedRestDestructuring => "TSR9010",
            UnimplementedStatementKind => "TSR9011",
            UnresolvableTypeAnnotation => "TSR9012",
            UnresolvableDestructuringTypeAnnotation => "TSR9013",
            UnresolvableTypeParameterConstraint => "TSR9014",
            InternalCheckerError => "TSR9101",
            NestingTooDeep => "TSR9102",

            UnreachableCode => "TSR1601",
            ExpressionTooComplex => "TSR1701",
        }
    }
}

impl DiagnosticCode {
    /// The tsc diagnostic number this code stands for, or `None` for a code that has
    /// no tsc counterpart (the 9000s: "not yet checked" markers, always warnings).
    ///
    /// Several variants share a number on purpose where tsc has only one: the three
    /// "type is not assignable" sites (declaration, return, static field) are all
    /// TS2322, because tsc does not tell them apart either.
    pub fn tsc_code(self) -> Option<u32> {
        use DiagnosticCode::*;
        Some(match self {
            BinaryOperandTypeMismatch => 2365,
            ArgumentNotAssignable => 2345,
            ReturnTypeMismatch => 2322,
            DeclaredTypeMismatch => 2322,
            DestructuringPatternTypeMismatch => 2322,
            StaticFieldInitializerMismatch => 2322,
            TypeArgumentConstraintViolation => 2344,
            ExcessProperty => 2353,
            PropertyNotInitialized => 2564,
            MissingProperty => 2741,
            MissingProperties => 2739,
            MissingPropertiesMany => 2740,
            ArithmeticLeftOperandInvalid => 2362,
            ArithmeticRightOperandInvalid => 2363,
            PossiblyNull => 18047,
            PossiblyUndefined => 18048,
            PossiblyNullOrUndefined => 18049,
            ObjectPossiblyNull => 2531,
            ObjectPossiblyUndefined => 2532,
            ObjectPossiblyNullOrUndefined => 2533,

            NotCallable => 2349,
            NotConstructor => 2351,
            ArgumentArityMismatch => 2554,
            ArgumentArityAtLeast => 2555,
            SpreadArgumentNotTupleOrRest => 2556,
            NoOverloadMatches => 2769,

            UnresolvedIdentifier => 2304,
            PropertyDoesNotExist => 2339,
            ModuleNotFound => 2307,
            NoExportedMember => 2305,
            NoDefaultExport => 1192,
            TypeUsedAsValue => 2693,

            ArrayDestructuringRequiresArray => 2488,

            TypeArgumentCountMismatch => 2314,
            TypeArgumentCountRange => 2707,
            TypeIsNotGeneric => 2315,
            DuplicateTypeDeclaration => 2300,
            EnumDeclarationMerge => 2567,

            ImplicitAnyParameter => 7006,
            ImplicitAnyElement => 7053,
            ImplicitAnyThis => 2683,

            UnreachableCode => 7027,
            ExpressionTooComplex => 2590,

            UnimplementedCallExpressionKind
            | UnimplementedNewExpressionTarget
            | UntypedParameterSkipsArityCheck
            | UnimplementedComputedMemberKey
            | UnimplementedOptionalChainLink
            | UnimplementedExpressionKind
            | UnimplementedStaticAccessor
            | UnimplementedClassShape
            | UnimplementedDestructuringKey
            | UnimplementedRestDestructuring
            | UnimplementedStatementKind
            | UnresolvableTypeAnnotation
            | UnresolvableDestructuringTypeAnnotation
            | UnresolvableTypeParameterConstraint
            | InternalCheckerError
            | NestingTooDeep => return None,
        })
    }

    /// `TS2322`-style text, or `None` where there is no tsc counterpart.
    pub fn tsc_str(self) -> Option<String> {
        self.tsc_code().map(|code| format!("TS{code}"))
    }
}

impl std::fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
