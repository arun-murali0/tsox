use oxc_ast::ast::{
    Class, ClassElement, Expression, MethodDefinitionKind, PropertyKey, TSInterfaceDeclaration,
    TSSignature, TSType,
};
use oxc_span::{GetSpan, Span};

use crate::arena::{TypeArena, TypeId};
use crate::fxhash::{FxHashMap, FxHasher};
use crate::portable::ImportedParam;
use crate::semantic::substitute_type_params;
use crate::type_annotation::{
    resolve_function_params, resolve_object_members, resolve_ts_type, resolve_type_annotation,
};
use crate::types::{FileId, ObjectType, PropertyEntry, Type, TypeParameterId};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

// The arguments an instantiation was made with, one (parameter, argument) pair per
// declared type parameter, in declaration order.
type Bindings = Vec<(TypeParameterId, TypeId)>;

// One memoized instantiation: the bindings it was built from, and the application that
// is the result.
type Instantiation = (Bindings, TypeId);

// A type namespace is a single flat map from name to declaration, not a scope tree.
// There is no block or module scoping for types in this checker; every top-level
// interface, type alias, and class shares one namespace. Generic type parameters
// are fit into this same flat model by temporarily shadowing a name (see
// push_type_params below) rather than motivating a real scope-tree rewrite.
// What kind of declaration is being added, for duplicate detection. A separate enum
// from DeclKind because a new declaration has no resolved form yet.
#[derive(Clone, Copy)]
enum NewKind {
    Alias,
    Interface,
    Class,
    Enum,
}

#[derive(Clone, Copy)]
enum DeclKind<'a> {
    TypeAlias(
        &'a TSType<'a>,
        Option<&'a oxc_ast::ast::TSTypeParameterDeclaration<'a>>,
    ),
    Interface(&'a TSInterfaceDeclaration<'a>),
    Class(&'a Class<'a>),

    Resolved,
}

// Opaque token returned by push_type_params and consumed by pop_type_params.
// Carries whatever each shadowed name previously pointed to, so restoring is
// self-contained and does not need the caller to remember which names were pushed.
pub struct TypeParamScope<'a> {
    saved: Vec<(String, Option<TypeEntry<'a>>)>,
    // How many entries this scope added to active_type_params, so popping it removes
    // exactly those and leaves any enclosing scope's in place.
    activated: usize,
}

struct TypeEntry<'a> {
    kind: DeclKind<'a>,
    resolved: Option<TypeId>,

    resolving: bool,
    // The namespaces this declaration was written inside, outermost first, as full
    // prefixes (`["A", "A.B"]` for `namespace A { namespace B { ... } }`). Resolving it
    // later, from anywhere, happens in this scope, so a sibling written unqualified is
    // found.
    scope: Vec<String>,
}

// A type reference whose type argument count disagrees with the declaration it
// names. Recorded during type resolution, which has no access to the diagnostics
// list, and drained into real diagnostics once checking finishes (see
// bridge::check_program), the same way implicit_any_params is.
pub struct TypeArgumentIssue {
    pub name: String,
    // Fewest type arguments a reference must give: parameters without a default.
    pub required: usize,
    // Most it may give: every declared parameter.
    pub expected: usize,
    pub span: Span,
}

/// Counts of what a namespace holds and how often its instantiation memo answered, for
/// benchmarks and regression reports. Counts only: the maps' allocator capacity is a
/// property of the hash table, better measured with a heap profiler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NamespaceStats {
    pub entries: usize,
    pub type_parameters: usize,
    pub merged_interfaces: usize,
    pub instantiations: usize,
    pub instantiation_hits: u64,
    pub instantiation_misses: u64,
}

pub struct TypeNamespace<'a> {
    // The file whose names this namespace resolves. Every generic parameter the
    // namespace declares carries it (see TypeParameterId), so two files checked against
    // shared semantic state cannot mistake each other's parameters for their own.
    file_id: FileId,

    entries: FxHashMap<String, TypeEntry<'a>>,

    // Set when the stack guard cut a resolution short, so the check can report it once.
    depth_cut: bool,

    // Project globals this file uses whose declaration could not be carried over. They
    // exist, so they are not reported as missing, but they have no type here.
    unmodelled_globals: std::collections::HashSet<String>,

    // The namespaces the checker is currently inside, as full prefixes, outermost first.
    // A declaration registered now is stored under its qualified name (`N.I`), and an
    // unqualified name is looked up innermost prefix first. Empty at the top of a file.
    scope: Vec<String>,

    // The type parameters whose declaration the checker is currently inside: those of
    // the function, class, interface or alias whose signature or body is being
    // resolved or checked, innermost last. A call made inside such a body may mention
    // them in the callee's type (calling a parameter typed `(x: T) => U` inside
    // `map<T, U>`), and they must stay as they are there instead of being inferred as
    // if the callee had declared them. See is_type_param_in_scope.
    active_type_params: Vec<TypeParameterId>,

    // Keyed by TypeParameterId (file and source position, see that type in types.rs)
    // rather than by name or by an AST pointer, so a generic function's signature,
    // resolved once up front, and its body, resolved later in a separate pass,
    // agree on the exact same GenericParameter node for T. Without this, a T[]
    // annotation inside the body would never match the parameter T came from.
    type_param_cache: FxHashMap<TypeParameterId, TypeId>,

    // The type parameters whose constraint is being resolved right now.
    //
    // Problem: `class C<T extends C = any> {}` crashed the process with a stack
    // overflow. Resolving T's constraint `C` needs C's defaults filled in, which
    // pushes C's own type parameters again (push_decl_type_params), which resolves
    // T's constraint `C` again, and so on. type_param_cache cannot stop it, because
    // an entry is only written after the constraint has resolved, so every pass
    // through the loop is a cache miss.
    // Picked: record the parameter here for the duration of its constraint
    // resolution, and let a re-entrant visit see it as an unconstrained parameter
    // instead of recursing. The outer call is still the one that caches the real
    // parameter, with its constraint.
    // Cost: the inner, unconstrained node is built and thrown away on every
    // re-entrant visit, which only happens for a constraint that names its own
    // declaration, so it is rare and cheap. tsc's own wording for the cycle is not
    // reproduced here yet; this keeps the checker alive, nothing more.
    resolving_constraints: Vec<TypeParameterId>,

    // The type parameters whose `= Default` is being resolved right now.
    //
    // Problem: `interface I<T = I> {}` (and the same on a class or type alias)
    // overflowed the stack. The default `I` is written without arguments, so
    // resolving it asks for I's defaults, which resolves T's default `I` again.
    // This is the same shape as resolving_constraints above, one step earlier in
    // the chain, and it does not pass through the constraint at all.
    // Picked: the same marker, kept per parameter. A re-entrant request for a
    // default that is already being resolved gets no default, which the caller
    // already handles as "unresolvable" and turns into the error type.
    // Cost: the circular default is not reported yet. tsc says TS2716 ("Type
    // parameter 'T' has a circular default"); this only keeps the checker alive.
    resolving_defaults: Vec<TypeParameterId>,

    // The generic declarations that came from another file, by the local name they are
    // bound to. They have no AST, so arity, arguments, defaults and bounds that a
    // local declaration reads from its `<T, ...>` list are read from here instead (see
    // type_annotation's TSTypeReference arm). The base shape is the entry's resolved
    // type; it is kept alongside so a local type parameter that happens to share the
    // name is never mistaken for the import (see imported_generic).
    imported_generics: FxHashMap<String, Rc<ImportedGeneric>>,

    // Untyped parameters found inside function *type annotations* such as
    // `(x) => number`. Type resolution has no access to the diagnostics list, so
    // they are collected here, deduplicated by source position (the same
    // annotation can be resolved more than once), and drained into real
    // diagnostics once checking finishes. See bridge::check_program.
    implicit_any_params: Vec<(String, Span)>,

    // Type parameters whose `extends` bound could not be resolved. The parameter
    // is then treated as unconstrained, which silently loses both the call-site
    // check and the use of the bound's members in the body, so it is recorded
    // here and reported as a warning once checking finishes, the same way
    // implicit_any_params is.
    unresolved_constraints: Vec<(String, Span)>,

    // Real TypeScript merges multiple `interface Foo { ... }` declarations for
    // the same name into one shape; declaring the same name as a class or a type
    // alias twice, by contrast, is a real error. Rather than change
    // DeclKind::Interface to hold a Vec (which would cost DeclKind its Copy
    // derive, and every one of its several by-value `entry.kind` reads elsewhere
    // in this file would need reworking to match by reference instead), the
    // first `interface Foo` for a name is kept exactly where it already was, in
    // entries as DeclKind::Interface(decl), and every declaration after the
    // first is appended here instead. resolve() below folds this in when the
    // kind is Interface, alongside decl itself, so nothing else has to know two
    // tables exist. A class or alias with a colliding name is unaffected: it
    // still just overwrites in entries -- a separate limitation that interface
    // merging does not address.
    merged_interface_parts: FxHashMap<String, Vec<&'a TSInterfaceDeclaration<'a>>>,

    // Names declared twice in a way TypeScript rejects, one entry per declaration
    // involved: the first declaration once, then every later one. tsc reports the
    // error on each occurrence, so reporting on both keeps the two checkers' output
    // comparable line for line. Kept here, not reported, because resolving names has no
    // access to the diagnostics list; bridge::declare drains it into real diagnostics.
    // Only collisions that are certainly illegal are recorded. See note_collision.
    declaration_collisions: Vec<(String, Span, bool)>,

    // Intersections that would distribute into too many members (TS2590), by the span of
    // the type that was written. Resolving an annotation has no diagnostics to write to, so
    // they wait here until the checker drains them, as the collisions above do.
    too_complex: Vec<Span>,

    // Where each name was first declared, so a later collision can point back at it,
    // and which names have already had that first declaration reported (a third
    // declaration must not report the first one again).
    declaration_spans: FxHashMap<String, Span>,
    // Inside a namespace a declaration is either exported (one symbol across every block
    // of the namespace) or local to its own block. Kept per name: (exported, block).
    declaration_visibility: FxHashMap<String, (bool, u32)>,
    scope_blocks: Vec<u32>,
    // Whether each enclosing namespace is itself exported; see effective_exported.
    scope_exported: Vec<bool>,
    block_counter: u32,
    declaring_exported: bool,
    reported_first_declaration: Vec<String>,

    // See TypeArgumentIssue. Deduplicated by source position, since the same
    // annotation can be resolved more than once.
    type_argument_issues: Vec<TypeArgumentIssue>,

    // Type arguments that do not satisfy the `extends` bound of the parameter
    // they were given to (`Box<number>` for `Box<T extends string>`): the
    // parameter's name, the argument's resolved type, the bound it failed, and
    // the argument's span. Collected here for the same reason as the issues
    // above and deduplicated the same way.
    constraint_violations: Vec<(String, TypeId, TypeId, Span)>,

    // Finished generic instantiations, `Box<number>` and the like, by the generic
    // shape they came from. Without it every reference would redo the substitution
    // and rebuild the display name, however many times the same instantiation was
    // written. See cache_instantiation for what may go in and why.
    //
    // Keyed by the shape and a fingerprint of the bindings, so a lookup hashes two
    // integers and never allocates a Vec to ask. The fingerprint only picks a bucket:
    // a hit is confirmed by comparing the stored bindings slice, so two different
    // binding lists that collide in 64 bits can never be mistaken for each other. The
    // bucket is still a list because of exactly that case, and in practice holds one
    // entry.
    instantiations: FxHashMap<(TypeId, u64), Vec<Instantiation>>,
    instantiation_hits: u64,
    instantiation_misses: u64,
}

/// A generic declaration imported from another file: its type parameters in this arena's
/// terms, and the base shape they are substituted into.
#[derive(Debug)]
pub struct ImportedGeneric {
    pub base: TypeId,
    pub params: Vec<ImportedParam>,
}

pub enum Resolution {
    Resolved(TypeId),

    Circular,
    NotFound,
}

fn binding_fingerprint(bindings: &[(TypeParameterId, TypeId)]) -> u64 {
    let mut hasher = FxHasher::default();
    bindings.hash(&mut hasher);
    hasher.finish()
}

impl<'a> TypeNamespace<'a> {
    pub fn new() -> Self {
        Self::with_file_id(FileId::ROOT)
    }

    pub fn with_file_id(file_id: FileId) -> Self {
        Self {
            file_id,
            entries: FxHashMap::default(),
            depth_cut: false,
            unmodelled_globals: std::collections::HashSet::new(),
            scope: Vec::new(),
            active_type_params: Vec::new(),
            merged_interface_parts: FxHashMap::default(),
            declaration_collisions: Vec::new(),
            too_complex: Vec::new(),
            declaration_spans: FxHashMap::default(),
            declaration_visibility: FxHashMap::default(),
            scope_blocks: Vec::new(),
            scope_exported: Vec::new(),
            block_counter: 0,
            declaring_exported: false,
            reported_first_declaration: Vec::new(),
            type_param_cache: FxHashMap::default(),
            resolving_constraints: Vec::new(),
            resolving_defaults: Vec::new(),
            imported_generics: FxHashMap::default(),
            implicit_any_params: Vec::new(),
            unresolved_constraints: Vec::new(),
            type_argument_issues: Vec::new(),
            constraint_violations: Vec::new(),
            instantiations: FxHashMap::default(),
            instantiation_hits: 0,
            instantiation_misses: 0,
        }
    }

    /// Whether `id` belongs to a declaration the checker is currently inside. Inference
    /// treats such a parameter as a fixed type, not something to solve for: `T` inside
    /// `map<T, U>` is one particular unknown type for the whole body.
    pub fn is_type_param_in_scope(&self, id: TypeParameterId) -> bool {
        self.active_type_params.contains(&id)
    }

    /// The GenericParameter node already allocated for `id`, if its declaration has been
    /// pushed before.
    pub fn type_param_node(&self, id: TypeParameterId) -> Option<TypeId> {
        self.type_param_cache.get(&id).copied()
    }

    pub fn file_id(&self) -> FileId {
        self.file_id
    }

    // The application stored for this instantiation, if any. It carries no name a caller
    // could change, so a hit is handed out as it is.
    pub fn cached_instantiation(
        &mut self,
        base: TypeId,
        bindings: &[(TypeParameterId, TypeId)],
    ) -> Option<TypeId> {
        let key = (base, binding_fingerprint(bindings));
        let hit = self
            .instantiations
            .get(&key)
            .and_then(|bucket| {
                bucket
                    .iter()
                    .find(|(stored, _)| stored.as_slice() == bindings)
            })
            .map(|&(_, application)| application);
        if hit.is_some() {
            self.instantiation_hits += 1;
        } else {
            self.instantiation_misses += 1;
        }
        hit
    }

    // Only a finished instantiation belongs here, and the caller owns that judgement: it
    // must have come from a substitution that actually changed `base`. An unchanged
    // result means the shape was still an unfinished Ref, and remembering it would hand
    // back the empty shape forever.
    pub fn cache_instantiation(
        &mut self,
        base: TypeId,
        bindings: Vec<(TypeParameterId, TypeId)>,
        application: TypeId,
    ) {
        let key = (base, binding_fingerprint(&bindings));
        self.instantiations
            .entry(key)
            .or_default()
            .push((bindings, application));
    }

    /// Counts for performance reports; see NamespaceStats.
    #[allow(
        clippy::disallowed_methods,
        reason = "a sum does not depend on iteration order"
    )]
    pub fn stats(&self) -> NamespaceStats {
        NamespaceStats {
            entries: self.entries.len(),
            type_parameters: self.type_param_cache.len(),
            merged_interfaces: self.merged_interface_parts.len(),
            instantiations: self.instantiations.values().map(Vec::len).sum(),
            instantiation_hits: self.instantiation_hits,
            instantiation_misses: self.instantiation_misses,
        }
    }

    /// Records that the project declares `name` globally but no type for it could be carried
    /// over.
    pub fn mark_unmodelled_global(&mut self, name: &str) {
        self.unmodelled_globals.insert(name.to_owned());
    }

    /// True when `name` is a project global whose declaration could not be carried over.
    pub fn is_unmodelled_global(&self, name: &str) -> bool {
        self.unmodelled_globals.contains(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(&*self.canonical(name))
    }

    /// Enters `namespace N { ... }`: what is declared until `pop_scope` is `N.x`, and a
    /// name written inside is looked up as `N.name` before `name`.
    pub fn push_scope(&mut self, name: &str) {
        let prefix = match self.scope.last() {
            Some(outer) => format!("{outer}.{name}"),
            None => name.to_string(),
        };
        self.scope.push(prefix);
        self.block_counter += 1;
        self.scope_blocks.push(self.block_counter);
        self.scope_exported.push(self.declaring_exported);
    }

    /// The namespaces being checked, outermost first, as full prefixes (`A`, `A.B`).
    pub fn scope_prefixes(&self) -> &[String] {
        &self.scope
    }

    pub fn pop_scope(&mut self) {
        self.scope.pop();
        self.scope_blocks.pop();
        self.scope_exported.pop();
    }

    // A declaration inside a namespace that is itself not exported belongs to that local
    // namespace, a different symbol from the exported one of the same name, so it counts as
    // local. The outermost namespace is in file scope, where exporting does not matter.
    fn effective_exported(&self) -> bool {
        self.declaring_exported && self.scope_exported.iter().skip(1).all(|exported| *exported)
    }

    /// Says whether the declaration about to be registered carries `export`.
    pub fn set_declaration_exported(&mut self, exported: bool) {
        self.declaring_exported = exported;
    }

    // The name a declaration is registered under in the current scope.
    fn qualify(&self, name: &str) -> String {
        match self.scope.last() {
            Some(prefix) => format!("{prefix}.{name}"),
            None => name.to_string(),
        }
    }

    // The registered name that `name` means here: the innermost enclosing namespace that
    // declares it, else `name` as written (a top-level name, or a qualified `N.I`).
    fn canonical<'n>(&self, name: &'n str) -> std::borrow::Cow<'n, str> {
        for prefix in self.scope.iter().rev() {
            let candidate = format!("{prefix}.{name}");
            if self.entries.contains_key(&candidate) {
                return std::borrow::Cow::Owned(candidate);
            }
        }
        std::borrow::Cow::Borrowed(name)
    }

    pub fn note_implicit_any_params(&mut self, params: &oxc_ast::ast::FormalParameters) {
        for param in &params.items {
            if param.type_annotation.is_some() || param.initializer.is_some() {
                continue;
            }
            let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &param.pattern else {
                continue;
            };
            if self
                .implicit_any_params
                .iter()
                .any(|(_, span)| span.start == id.span.start)
            {
                continue;
            }
            self.implicit_any_params
                .push((id.name.to_string(), id.span));
        }
    }

    pub fn take_implicit_any_params(&mut self) -> Vec<(String, Span)> {
        std::mem::take(&mut self.implicit_any_params)
    }

    pub fn take_unresolved_constraints(&mut self) -> Vec<(String, Span)> {
        std::mem::take(&mut self.unresolved_constraints)
    }

    // The `<...>` list declared by the interface or type alias called `name`.
    // Outer None: not something whose type parameters are modelled (a class,
    // whose generics are not supported yet, a type parameter, an enum, or an
    // unknown name), so nothing is ever reported against it and an unsupported
    // feature cannot produce a false error. Inner None: a non-generic one.
    fn declared_type_param_list(
        &self,
        name: &str,
    ) -> Option<Option<&'a oxc_ast::ast::TSTypeParameterDeclaration<'a>>> {
        let name = &*self.canonical(name);
        match self.entries.get(name)?.kind {
            DeclKind::TypeAlias(_, type_params) => Some(type_params),
            DeclKind::Interface(decl) => Some(
                decl.type_parameters
                    .as_deref()
                    .or_else(|| self.merged_type_parameters(name)),
            ),
            DeclKind::Class(class) => Some(class.type_parameters.as_deref()),
            DeclKind::Resolved => None,
        }
    }

    // A merged interface's own `<T, ...>` list, from whichever part (if any)
    // happens to declare one. Real TypeScript requires every merged part that
    // declares type parameters to declare the identical list; this does not
    // check that and just takes the first one found, which is the right answer
    // for the common case (only one part is generic) and a reasonable one
    // otherwise, since nothing here can express "two declarations must agree"
    // as an error yet.
    fn merged_type_parameters(
        &self,
        name: &str,
    ) -> Option<&'a oxc_ast::ast::TSTypeParameterDeclaration<'a>> {
        self.merged_interface_parts
            .get(name)?
            .iter()
            .find_map(|part| part.type_parameters.as_deref())
    }

    // (required, total) type argument counts. A parameter with a default may be
    // omitted, which is why these differ; tsc accepts `Pair<number>` for
    // `interface Pair<A, B = string>`.
    pub fn declared_type_param_arity(&self, name: &str) -> Option<(usize, usize)> {
        if let Some(imported) = self.imported_generic(name) {
            return Some((
                imported
                    .params
                    .iter()
                    .filter(|p| p.default.is_none())
                    .count(),
                imported.params.len(),
            ));
        }
        Some(match self.declared_type_param_list(name)? {
            None => (0, 0),
            Some(decl) => (
                decl.params
                    .iter()
                    .filter(|param| param.default.is_none())
                    .count(),
                decl.params.len(),
            ),
        })
    }

    // The declaration itself when it is generic, so a reference can bind one
    // argument per *declared* parameter, in declaration order, instead of
    // guessing the order from whichever parameters the resolved body mentions.
    pub fn declared_type_param_decl(
        &self,
        name: &str,
    ) -> Option<&'a oxc_ast::ast::TSTypeParameterDeclaration<'a>> {
        self.declared_type_param_list(name)?
            .filter(|decl| !decl.params.is_empty())
    }

    // Resolves the `= Default` of the type parameter at `index`, with the
    // declaration's own parameters in scope so a default may mention an earlier
    // one (`<T, U = T>`). The result can still contain those GenericParameter
    // placeholders; the caller substitutes the arguments it already has.
    pub fn resolve_type_param_default(
        &mut self,
        arena: &mut TypeArena,
        decl: &oxc_ast::ast::TSTypeParameterDeclaration<'_>,
        index: usize,
    ) -> Option<TypeId> {
        let param = decl.params.get(index)?;
        let default = param.default.as_ref()?;
        // Already resolving this very default further up the stack: it names its own
        // declaration (see resolving_defaults), so there is no answer to give.
        let id = TypeParameterId::with_file(self.file_id, param.span().start, index as u32);
        if self.resolving_defaults.contains(&id) {
            return None;
        }
        self.resolving_defaults.push(id);
        let scope = self.push_decl_type_params(arena, Some(decl));
        let resolved = resolve_ts_type(default, self, arena);
        self.pop_type_params(scope);
        self.resolving_defaults.pop();
        resolved
    }

    pub fn note_type_argument_issue(
        &mut self,
        name: &str,
        required: usize,
        expected: usize,
        span: Span,
    ) {
        if self
            .type_argument_issues
            .iter()
            .any(|issue| issue.span.start == span.start)
        {
            return;
        }
        self.type_argument_issues.push(TypeArgumentIssue {
            name: name.to_string(),
            required,
            expected,
            span,
        });
    }

    // The resolved `extends` bound of a declared type parameter, read back from
    // the GenericParameter node push_decl_type_params cached for it. None when the
    // parameter is unconstrained, or its bound could not be resolved (already
    // reported as a warning), so nothing is ever enforced against it.
    pub fn type_param_constraint(&self, arena: &TypeArena, id: TypeParameterId) -> Option<TypeId> {
        let node = *self.type_param_cache.get(&id)?;
        match arena.get(node) {
            Type::GenericParameter(_, _, constraint) => *constraint,
            _ => None,
        }
    }

    pub fn note_constraint_violation(
        &mut self,
        parameter_name: &str,
        actual: TypeId,
        constraint: TypeId,
        span: Span,
    ) {
        if self
            .constraint_violations
            .iter()
            .any(|(_, _, _, existing)| existing.start == span.start)
        {
            return;
        }
        self.constraint_violations
            .push((parameter_name.to_string(), actual, constraint, span));
    }

    pub fn take_constraint_violations(&mut self) -> Vec<(String, TypeId, TypeId, Span)> {
        std::mem::take(&mut self.constraint_violations)
    }

    pub fn take_type_argument_issues(&mut self) -> Vec<TypeArgumentIssue> {
        std::mem::take(&mut self.type_argument_issues)
    }

    // The flag says an enum is one side of the collision, which tsc words differently
    // (TS2567) from two other declarations of one name (TS2300).
    // A type that is written once can be resolved more than once (an alias is resolved
    // again each time something asks for a fresh view of it), so the same span is noted
    // at most once and tsc's single TS2590 is reported once.
    pub fn note_too_complex(&mut self, span: Span) {
        if !self.too_complex.contains(&span) {
            self.too_complex.push(span);
        }
    }

    pub fn take_too_complex(&mut self) -> Vec<Span> {
        std::mem::take(&mut self.too_complex)
    }

    pub fn take_declaration_collisions(&mut self) -> Vec<(String, Span, bool)> {
        std::mem::take(&mut self.declaration_collisions)
    }

    // Records `name` as declared twice when what is already registered under it cannot
    // legally coexist with the new declaration. TypeScript allows an interface to merge
    // with another interface or with a class, and an enum with another enum; this
    // checker does not model the last two merges (the later declaration still replaces
    // the earlier entry), so flagging them would report an error tsc does not. Only
    // pairs that are an error in TypeScript are recorded: an alias on either side, two
    // classes, or an enum against anything but an enum.
    //
    // Problem: two blocks of one namespace share a registered name, so a class that is not
    // exported from the second block (`namespace A { export class P {} }` then
    // `namespace A { class P {} }`) looked like a redeclaration of the exported one.
    // Picked: inside a namespace, two declarations collide when both are exported (they are
    // one symbol across blocks) or both are local to the same block. A local one never
    // collides with an exported one or with a local one in another block.
    // Cost: an exported against a local declaration in one block (tsc's TS2395) is not
    // reported.
    fn note_collision(&mut self, name: &str, new_kind: NewKind, span: Span) {
        if let (Some(&block), Some(&(was_exported, was_block))) = (
            self.scope_blocks.last(),
            self.declaration_visibility.get(name),
        ) {
            let exported = self.effective_exported();
            let same_symbol = was_exported == exported && (exported || was_block == block);
            if !same_symbol {
                return;
            }
        }
        let Some(existing) = self.entries.get(name) else {
            return;
        };
        let illegal = match (existing.kind, new_kind) {
            (DeclKind::TypeAlias(..), _) | (_, NewKind::Alias) => true,
            (DeclKind::Class(_), NewKind::Class) => true,
            (DeclKind::Resolved, NewKind::Enum) => false,
            (DeclKind::Resolved, _) | (_, NewKind::Enum) => true,
            (DeclKind::Interface(_), NewKind::Interface | NewKind::Class) => false,
            (DeclKind::Class(_), NewKind::Interface) => false,
        };
        if !illegal {
            return;
        }
        let involves_enum =
            matches!(existing.kind, DeclKind::Resolved) || matches!(new_kind, NewKind::Enum);
        if let Some(&first) = self.declaration_spans.get(name)
            && !self
                .reported_first_declaration
                .iter()
                .any(|seen| seen == name)
        {
            self.declaration_collisions
                .push((name.to_string(), first, involves_enum));
            self.reported_first_declaration.push(name.to_string());
        }
        self.declaration_collisions
            .push((name.to_string(), span, involves_enum));
    }

    // Remembers where `name` was first declared. Called after note_collision, so the
    // declaration being added is never mistaken for the earlier one it collides with.
    fn remember_declaration(&mut self, name: &str, span: Span) {
        let exported = self.effective_exported();
        if let Some(&block) = self.scope_blocks.last() {
            self.declaration_visibility
                .entry(name.to_string())
                .or_insert((exported, block));
        }
        self.declaration_spans
            .entry(name.to_string())
            .or_insert(span);
    }

    pub fn insert_type_alias(
        &mut self,
        name: &str,
        body: &'a TSType<'a>,
        type_parameters: Option<&'a oxc_ast::ast::TSTypeParameterDeclaration<'a>>,
        span: Span,
    ) {
        let qualified = self.qualify(name);
        let name = qualified.as_str();
        self.note_collision(name, NewKind::Alias, span);
        self.remember_declaration(name, span);
        self.entries.insert(
            name.to_string(),
            TypeEntry {
                kind: DeclKind::TypeAlias(body, type_parameters),
                resolved: None,
                resolving: false,
                scope: self.scope.clone(),
            },
        );
    }

    pub fn insert_interface(&mut self, name: &str, decl: &'a TSInterfaceDeclaration<'a>) {
        let qualified = self.qualify(name);
        let name = qualified.as_str();
        // A second (or later) `interface Foo` for a name already holding one is
        // declaration merging, not a redeclaration: it adds to the first part
        // rather than replacing it. See the doc comment on
        // merged_interface_parts for why this is stored separately instead of
        // changing what DeclKind::Interface itself holds.
        let already_an_interface = matches!(
            self.entries.get(name),
            Some(entry) if matches!(entry.kind, DeclKind::Interface(_))
        );
        if already_an_interface {
            self.merged_interface_parts
                .entry(name.to_string())
                .or_default()
                .push(decl);
            return;
        }
        self.note_collision(name, NewKind::Interface, decl.id.span);
        self.remember_declaration(name, decl.id.span);
        self.entries.insert(
            name.to_string(),
            TypeEntry {
                kind: DeclKind::Interface(decl),
                resolved: None,
                resolving: false,
                scope: self.scope.clone(),
            },
        );
    }

    pub fn insert_class(&mut self, name: &str, class: &'a Class<'a>) {
        let qualified = self.qualify(name);
        let name = qualified.as_str();
        let span = class.id.as_ref().map_or_else(|| class.span(), |id| id.span);
        self.note_collision(name, NewKind::Class, span);
        self.remember_declaration(name, span);
        self.entries.insert(
            name.to_string(),
            TypeEntry {
                kind: DeclKind::Class(class),
                resolved: None,
                resolving: false,
                scope: self.scope.clone(),
            },
        );
    }

    // An enum is registered already resolved, since its member types are built
    // eagerly. Unlike insert_resolved it is a user-written declaration, so it takes
    // part in duplicate detection.
    pub fn insert_enum(&mut self, name: &str, type_id: TypeId, span: Span) {
        let qualified = self.qualify(name);
        let name = qualified.as_str();
        self.note_collision(name, NewKind::Enum, span);
        self.remember_declaration(name, span);
        self.insert_resolved(name, type_id);
    }

    // Binds `name` to a declaration imported from another file. A non-generic one is just
    // a resolved name. A generic one also records its parameters, and their nodes go in
    // the parameter cache so a bound is enforced on an explicit argument the same way
    // it is for a local declaration (type_param_constraint reads it from there).
    pub fn insert_imported(&mut self, name: &str, type_id: TypeId, params: Vec<ImportedParam>) {
        self.insert_resolved(name, type_id);
        if params.is_empty() {
            self.imported_generics.remove(name);
            return;
        }
        for param in &params {
            self.type_param_cache.insert(param.id, param.node);
        }
        self.imported_generics.insert(
            name.to_string(),
            Rc::new(ImportedGeneric {
                base: type_id,
                params,
            }),
        );
    }

    // The imported generic bound to `name`, if that is still what the name means. A type
    // parameter shadows a name by replacing its entry, so the entry's type is compared
    // with the one recorded at import time.
    pub fn imported_generic(&self, name: &str) -> Option<Rc<ImportedGeneric>> {
        let name = &*self.canonical(name);
        let imported = self.imported_generics.get(name)?;
        let current = self.entries.get(name)?.resolved?;
        (current == imported.base).then(|| Rc::clone(imported))
    }

    pub fn insert_resolved(&mut self, name: &str, type_id: TypeId) {
        let qualified = self.qualify(name);
        let name = qualified.as_str();
        self.entries.insert(
            name.to_string(),
            TypeEntry {
                kind: DeclKind::Resolved,
                resolved: Some(type_id),
                resolving: false,
                scope: self.scope.clone(),
            },
        );
    }

    // Temporarily shadows each of func's own type parameter names with a
    // GenericParameter node, so a generic function's signature and body can refer
    // to T and have it resolve here just like any other named type. The same node
    // is reused across repeated calls for the same function through
    // type_param_cache, and whatever a name previously pointed to is restored by
    // the matching pop_type_params call.
    //
    // `func` deliberately uses `'_` rather than `'a`: only names and spans are read
    // from it, nothing borrowed from the AST is stored. Tying it to `'a` would
    // force every caller to hold a Function with exactly the namespace's lifetime,
    // because `&mut TypeNamespace<'a>` is invariant in `'a`.
    pub fn push_type_params(
        &mut self,
        arena: &mut TypeArena,
        func: &oxc_ast::ast::Function<'_>,
    ) -> TypeParamScope<'a> {
        self.push_decl_type_params(arena, func.type_parameters.as_deref())
    }

    // The declaration-shaped generalization of push_type_params above: a
    // function's own `<T, ...>` list is one source of a type parameter
    // declaration, but a generic interface (`interface Box<T>`) or generic
    // type alias (`type Box<T> = ...`) needs the exact same shadow-then-resolve
    // treatment for its body to be able to refer to T. Both routes end up
    // allocating the same kind of GenericParameter node, keyed the same way by
    // TypeParameterId, so a interface's declared T and a type alias's declared
    // T are never confused with each other (different declaration_span_start)
    // even though both might be named "T".
    pub fn push_decl_type_params(
        &mut self,
        arena: &mut TypeArena,
        decl: Option<&oxc_ast::ast::TSTypeParameterDeclaration<'_>>,
    ) -> TypeParamScope<'a> {
        let Some(decl) = decl else {
            return TypeParamScope {
                saved: Vec::new(),
                activated: 0,
            };
        };

        let mut saved = Vec::with_capacity(decl.params.len());
        for (index, param) in decl.params.iter().enumerate() {
            let name = param.name.name.to_string();
            let id = TypeParameterId::with_file(self.file_id, param.span().start, index as u32);

            // Resolved before touching the cache entry, not inside its
            // or_insert_with closure: resolving a constraint needs a full &mut
            // self (it can reference other named types), which would conflict
            // with the field-level borrow entry() already holds on
            // type_param_cache. Only resolved on a genuine cache miss, so a
            // constraint referencing something expensive is still only resolved
            // once per declaration, not once per push_type_params call.
            let type_id = match self.type_param_cache.get(&id).copied() {
                Some(cached) => cached,
                // Re-entered while this very parameter's constraint is still being
                // resolved (see resolving_constraints). Not cached: the outer call
                // owns the cache entry and will write the constrained node.
                None if self.resolving_constraints.contains(&id) => {
                    arena.alloc(Type::GenericParameter(id, name.clone(), None))
                }
                None => {
                    self.resolving_constraints.push(id);
                    let constraint = param
                        .constraint
                        .as_ref()
                        .and_then(|c| crate::type_annotation::resolve_ts_type(c, self, arena));
                    self.resolving_constraints.pop();
                    if param.constraint.is_some()
                        && constraint.is_none()
                        && !self
                            .unresolved_constraints
                            .iter()
                            .any(|(_, span)| span.start == param.span().start)
                    {
                        self.unresolved_constraints
                            .push((name.clone(), param.span()));
                    }
                    let type_id = arena.alloc(Type::GenericParameter(id, name.clone(), constraint));
                    self.type_param_cache.insert(id, type_id);
                    type_id
                }
            };

            saved.push((name.clone(), self.entries.remove(&name)));
            self.insert_resolved(&name, type_id);
            self.active_type_params.push(id);
        }
        let activated = saved.len();
        TypeParamScope { saved, activated }
    }

    pub fn pop_type_params(&mut self, scope: TypeParamScope<'a>) {
        let keep = self
            .active_type_params
            .len()
            .saturating_sub(scope.activated);
        self.active_type_params.truncate(keep);
        for (name, saved_entry) in scope.saved {
            match saved_entry {
                Some(entry) => {
                    self.entries.insert(name, entry);
                }
                None => {
                    self.entries.remove(&name);
                }
            }
        }
    }

    // Resolution is lazy and memoized: a declaration is only turned into a
    // concrete Type the first time something actually references it, and the
    // result is cached in entry.resolved so later references are free. The
    // resolving flag detects a declaration that references itself while still
    // being resolved (for example, two type aliases that refer to each other),
    // reporting it as Circular instead of recursing forever.
    /// Records that the stack guard cut a walk short, for the check to report once.
    pub(crate) fn note_depth_cut(&mut self) {
        self.depth_cut = true;
    }

    /// True once, if the stack guard cut a resolution short while checking this file.
    pub fn take_depth_cut(&mut self) -> bool {
        std::mem::take(&mut self.depth_cut)
    }

    pub fn resolve(&mut self, name: &str, arena: &mut TypeArena) -> Resolution {
        // A chain of declarations that each name the next can be as long as the file, and
        // each link is a recursive call. Past the stack budget the answer is the error
        // type, an unknown that is cached like any other result. Answering "circular"
        // instead was not cached, so every later declaration in the chain walked it all
        // again until the budget ran out: minutes for a chain of a few tens of thousands.
        if crate::stack_guard::exhausted() {
            self.depth_cut = true;
            return Resolution::Resolved(arena.error());
        }
        let registered = self.canonical(name).into_owned();
        // The declaration is read in the scope it was written in, whoever asks.
        let entry_scope = match self.entries.get(&registered) {
            Some(entry) => entry.scope.clone(),
            None => return Resolution::NotFound,
        };
        let outer = std::mem::replace(&mut self.scope, entry_scope);
        let resolution = self.resolve_entry(&registered, arena);
        self.scope = outer;
        resolution
    }

    fn resolve_entry(&mut self, name: &str, arena: &mut TypeArena) -> Resolution {
        let Some(entry) = self.entries.get(name) else {
            return Resolution::NotFound;
        };

        if let Some(type_id) = entry.resolved {
            return Resolution::Resolved(type_id);
        }
        if entry.resolving {
            return Resolution::Circular;
        }

        let kind = entry.kind;

        // A generic interface or type alias's own `<T, ...>` list is pushed as a
        // shadow, exactly like a generic function's, before its body is resolved,
        // so `Box<T>`'s body can refer to T and have it resolve to the same
        // GenericParameter node a caller will later substitute via
        // type_annotation::resolve_ts_type's TSTypeReference arm. The resulting
        // resolved shape -- still containing bare GenericParameter placeholders
        // -- is what gets cached in entry.resolved; substitution for a specific
        // `Box<number>` happens later, per reference, against this one cached
        // generic shape, the same way a generic function's FunctionType is
        // resolved once and substituted per call.
        let type_params = match kind {
            DeclKind::TypeAlias(_, type_params) => type_params,
            DeclKind::Interface(decl) => decl
                .type_parameters
                .as_deref()
                .or_else(|| self.merged_type_parameters(name)),
            // A class's own `<T, ...>` list is shadowed the same way an
            // interface's or alias's is, so a field or method typed `T` (or
            // referring to the class's own name generically, `next: Box<T>`)
            // resolves inside resolve_class the same way. Substitution for a
            // specific `Box<number>` happens later, per reference, exactly as
            // it does for an interface -- resolve_class itself needs no
            // change at all for this.
            DeclKind::Class(class) => class.type_parameters.as_deref(),
            DeclKind::Resolved => None,
        };

        // An interface, a class, and a type-literal alias (`type X = { ... }`)
        // are always object-shaped, so a property inside one can legitimately
        // refer back to the declaration itself -- a linked list's
        // `next: Node | null`, a tree's `children: Node[]`. For these, a Ref
        // to the declaration is registered as this name's resolved type
        // *before* its members are resolved, so the self-reference finds a real
        // TypeId (via the entry.resolved short-circuit above) instead of
        // hitting the Circular case below. That case stays reserved for a bare
        // alias chain that never bottoms out in an object shape (`type A = A;`,
        // which really is invalid TypeScript and stays unresolvable here too).
        let self_referenceable = matches!(kind, DeclKind::Interface(_) | DeclKind::Class(_))
            || matches!(kind, DeclKind::TypeAlias(body, _) if matches!(body, TSType::TSTypeLiteral(_)));

        let reference = if self_referenceable {
            let id = arena.alloc_ref();
            if let Some(entry) = self.entries.get_mut(name) {
                entry.resolved = Some(id);
            }
            Some(id)
        } else {
            if let Some(entry) = self.entries.get_mut(name) {
                entry.resolving = true;
            }
            None
        };

        let scope = self.push_decl_type_params(arena, type_params);

        let resolved =
            match kind {
                DeclKind::TypeAlias(body, _) => resolve_ts_type(body, self, arena),
                DeclKind::Interface(decl) => {
                    // Declaration merging: every part's members are resolved
                    // together as one shape, decl's own first, then each merged
                    // part's in the order they were declared. A property name
                    // repeated across parts is not specially detected -- it hits
                    // resolve_object_members' existing duplicate-name check, the
                    // same one that already applies within a single interface, and
                    // makes the whole merged interface unresolvable, which is safe
                    // (if imprecise) rather than silently picking one.
                    let merged = self.merged_interface_parts.get(name);
                    let members: Vec<&TSSignature> =
                        decl.body
                            .body
                            .iter()
                            .chain(merged.into_iter().flat_map(|parts| {
                                parts.iter().flat_map(|part| part.body.body.iter())
                            }))
                            .collect();
                    resolve_object_members(&members, self, arena)
                }
                DeclKind::Class(class) => self.resolve_class(class, arena),
                DeclKind::Resolved => None,
            };
        self.pop_type_params(scope);

        let Some(entry) = self.entries.get_mut(name) else {
            return Resolution::NotFound;
        };
        entry.resolving = false;

        // A non-generic interface, alias or class prints as its own name from
        // here on ("Dog", not its member list). A generic one is left unnamed:
        // this cached shape still holds bare GenericParameter placeholders and
        // is never itself the type of anything a person sees -- each
        // instantiation (type_annotation::resolve_ts_type's TSTypeReference
        // arm) names its own substituted result instead ("Box<number>").
        let is_non_generic = type_params.is_none_or(|params| params.params.is_empty());

        match (reference, resolved) {
            (Some(reference), Some(body)) => {
                // resolve_object_members and resolve_class allocate an Object
                // (through alloc(), so it may be an id shared with an identical
                // anonymous shape), and that is fine as the body: the id of the
                // declaration itself is the Ref, which a self-reference (and
                // entry.resolved) already points to, and it now reads as that body.
                arena.resolve_ref(reference, body);
                entry.resolved = Some(reference);
                if is_non_generic {
                    arena.name_ref(reference, name);
                }
                Resolution::Resolved(reference)
            }
            (Some(reference), None) => {
                // A member turned out to be unresolvable after all. A failed
                // resolution's partial work is discarded, never handed to a
                // caller, so nothing outside this call could have captured the
                // Ref as real by now -- safe to just forget it.
                arena.fail_ref(reference);
                entry.resolved = None;
                Resolution::NotFound
            }
            (None, Some(type_id)) => {
                // `type Scores = number[]` resolves to the id every `number[]`
                // shares. The alias's name goes on a wrapper around that id, so the
                // shared id is left as it is and no other array of numbers prints as
                // "Scores". A primitive is not wrapped, so `type Age = number` still
                // prints as `number`.
                let type_id = if is_non_generic {
                    let slot = arena.alloc_name(name);
                    arena.alloc_named(slot, type_id)
                } else {
                    type_id
                };
                entry.resolved = Some(type_id);
                Resolution::Resolved(type_id)
            }
            (None, None) => Resolution::NotFound,
        }
    }

    /// Resolves the instance shape of an anonymous class expression/declaration for
    /// export serialization. It has no name to register in `entries`, but its generic
    /// parameters still use the same file-and-span identity cache as named classes.
    pub(crate) fn resolve_anonymous_class(
        &mut self,
        class: &'a Class<'a>,
        arena: &mut TypeArena,
    ) -> Option<TypeId> {
        let scope = self.push_decl_type_params(arena, class.type_parameters.as_deref());
        let resolved = self.resolve_class(class, arena);
        self.pop_type_params(scope);
        resolved
    }

    // Builds a class's structural shape by first copying the parent's own already
    // resolved properties, then overriding or adding this class's own fields and
    // methods. This clones the parent's whole property list at every level of an
    // inheritance chain, so resolving a class costs work proportional to its own
    // depth in the hierarchy, not just its own member count. PropertyEntry's name
    // is an Rc<str> specifically so that repeated clone is a refcount bump rather
    // than a fresh heap allocation per property per level.
    fn resolve_class(&mut self, class: &'a Class<'a>, arena: &mut TypeArena) -> Option<TypeId> {
        let mut properties: Vec<PropertyEntry> = Vec::new();

        if let Some(heritage) = &class.heritage {
            if let Expression::Identifier(parent_name) = &heritage.expression {
                if let Resolution::Resolved(parent_type) = self.resolve(&parent_name.name, arena) {
                    // `extends Animal<string>`: the parent's own declared type
                    // parameters (if any) are bound to the arguments given here,
                    // one per declared parameter in order, the same way a type
                    // reference like `Box<number>` substitutes in
                    // type_annotation.rs. A parent with no type parameters, or
                    // heritage with no `<...>` at all, just uses the parent's
                    // shape as-is -- the common, non-generic case.
                    let parent_type = match self.declared_type_param_decl(&parent_name.name) {
                        Some(decl) => {
                            let mut bindings: Bindings = Vec::with_capacity(decl.params.len());
                            for (index, param) in decl.params.iter().enumerate() {
                                let parameter_id = TypeParameterId::with_file(
                                    self.file_id,
                                    param.span().start,
                                    index as u32,
                                );
                                let bound = heritage
                                    .type_arguments
                                    .as_ref()
                                    .and_then(|args| args.params.get(index))
                                    .and_then(|arg| resolve_ts_type(arg, self, arena))
                                    .unwrap_or_else(|| arena.error());
                                bindings.push((parameter_id, bound));
                            }
                            substitute_type_params(arena, parent_type, &bindings)
                        }
                        None => parent_type,
                    };
                    if let Type::Object(parent_object) = arena.get(parent_type) {
                        properties = parent_object.properties.to_vec();
                    }
                }
            }
        }

        for element in &class.body.body {
            match element {
                // A computed field name, or any field/method this checker cannot
                // fully resolve, makes the whole class unresolvable rather than
                // just skipping that one member. Silently dropping a member would
                // let later code reference a property that source code clearly
                // declares but that structurally does not exist here, which is a
                // worse failure mode than an honest "this class isn't supported."
                ClassElement::PropertyDefinition(prop) if !prop.r#static => {
                    let PropertyKey::StaticIdentifier(key) = &prop.key else {
                        return None;
                    };
                    let annotation = prop.type_annotation.as_ref()?;
                    let type_id = resolve_type_annotation(annotation, self, arena)?;
                    upsert_property(
                        &mut properties,
                        key.name.to_string(),
                        type_id,
                        prop.optional,
                        false,
                    );
                }
                ClassElement::MethodDefinition(method)
                    if !method.r#static && method.kind == MethodDefinitionKind::Method =>
                {
                    let PropertyKey::StaticIdentifier(key) = &method.key else {
                        return None;
                    };
                    let method_scope = self.push_type_params(arena, &method.value);
                    let return_type = method
                        .value
                        .return_type
                        .as_ref()
                        .and_then(|rt| resolve_type_annotation(rt, self, arena));

                    let (params, is_untyped) =
                        match resolve_function_params(&method.value.params, self, arena) {
                            Some(params) => (params, false),
                            None => (Vec::new(), true),
                        };
                    let type_params = crate::type_annotation::resolve_function_type_params(
                        method.value.type_parameters.as_deref(),
                        self,
                        arena,
                    );
                    self.pop_type_params(method_scope);
                    let Some(return_type) = return_type else {
                        return None;
                    };
                    let method_type = arena.alloc(Type::Function(crate::types::FunctionType {
                        params,
                        return_type,
                        type_params,
                        is_untyped,
                    }));
                    upsert_property(
                        &mut properties,
                        key.name.to_string(),
                        method_type,
                        false,
                        true,
                    );
                }

                // Problem: `get total(): number { ... }` was dropped without a word, so
                // `order.total` reported a missing property even though the class
                // plainly declares it.
                // Picked: a getter is an ordinary property of its return type. A setter
                // alone is a property of its parameter type, and when both exist the
                // getter's type wins. As with a field, an accessor this checker cannot
                // type (no return annotation, a computed name) makes the class
                // unresolvable instead of dropping a member the source declares.
                // Cost: nothing marks the property read-only when there is no setter,
                // and the accessor bodies are not checked yet.
                ClassElement::MethodDefinition(method)
                    if !method.r#static && method.kind == MethodDefinitionKind::Get =>
                {
                    let PropertyKey::StaticIdentifier(key) = &method.key else {
                        return None;
                    };
                    let type_id = method
                        .value
                        .return_type
                        .as_ref()
                        .and_then(|rt| resolve_type_annotation(rt, self, arena))?;
                    upsert_property(&mut properties, key.name.to_string(), type_id, false, false);
                }
                ClassElement::MethodDefinition(method)
                    if !method.r#static && method.kind == MethodDefinitionKind::Set =>
                {
                    let PropertyKey::StaticIdentifier(key) = &method.key else {
                        return None;
                    };
                    let has_getter = class.body.body.iter().any(|other| {
                        matches!(
                            other,
                            ClassElement::MethodDefinition(getter)
                                if !getter.r#static
                                    && getter.kind == MethodDefinitionKind::Get
                                    && matches!(&getter.key, PropertyKey::StaticIdentifier(k) if k.name == key.name)
                        )
                    });
                    if !has_getter {
                        let param = method.value.params.items.first()?;
                        let annotation = param.type_annotation.as_ref()?;
                        let type_id = resolve_type_annotation(annotation, self, arena)?;
                        upsert_property(
                            &mut properties,
                            key.name.to_string(),
                            type_id,
                            false,
                            false,
                        );
                    }
                }

                // `constructor(public x: number, readonly y: string)` declares the
                // instance properties `x` and `y` as well as the parameters. A parameter
                // with no modifier is only a parameter. As with a field, a parameter
                // property this checker cannot type (no annotation, or a destructured
                // pattern) makes the class unresolvable instead of silently dropping a
                // property the source plainly declares.
                ClassElement::MethodDefinition(method)
                    if method.kind == MethodDefinitionKind::Constructor =>
                {
                    for param in &method.value.params.items {
                        if param.accessibility.is_none() && !param.readonly {
                            continue;
                        }
                        let name = crate::type_annotation::binding_name(&param.pattern)?;
                        let annotation = param.type_annotation.as_ref()?;
                        let type_id = resolve_type_annotation(annotation, self, arena)?;
                        upsert_property(
                            &mut properties,
                            name.to_string(),
                            type_id,
                            param.optional,
                            false,
                        );
                    }
                }

                _ => {}
            }
        }

        Some(arena.alloc(Type::Object(ObjectType::new(properties))))
    }
}

fn upsert_property(
    properties: &mut Vec<PropertyEntry>,
    name: String,
    type_id: TypeId,
    optional: bool,
    is_method: bool,
) {
    match properties.iter_mut().find(|p| *p.name == *name) {
        Some(existing) => {
            existing.type_id = type_id;
            existing.optional = optional;
            existing.is_method = is_method;
        }
        None => properties.push(PropertyEntry {
            name: name.into(),
            type_id,
            optional,
            is_method,
        }),
    }
}

impl Default for TypeNamespace<'_> {
    fn default() -> Self {
        Self::new()
    }
}
