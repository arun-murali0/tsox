//! Instantiation: substituting type parameters, the depth budget, and the free-parameter
//! analysis that tells whether a type still depends on one.
use super::*;

// Rebuilds type_id, replacing every GenericParameter occurrence with its bound
// type, or unknown for a parameter nothing ever informed. Bails out immediately
// when type_id contains no type parameters at all, which is every ordinary,
// non-generic type, so this costs nothing beyond one quick tree walk for the
// common case. The function's own stored signature is never mutated; every call
// with different bindings produces a fresh, independent result.
pub fn substitute_type_params(
    arena: &mut TypeArena,
    type_id: TypeId,
    bindings: &[(crate::types::TypeParameterId, TypeId)],
) -> TypeId {
    substitute_impl(arena, type_id, bindings, false)
}

// Like substitute_type_params, but a parameter with no binding is left as it is
// instead of becoming unknown. A type reference such as `Box<number>` binds only
// the declaration's own parameters, so a method with a parameter of its own
// (`map<U>(f: (x: T) => U): U`) must keep its U to be inferred at the call site.
pub fn substitute_bound_type_params(
    arena: &mut TypeArena,
    type_id: TypeId,
    bindings: &[(crate::types::TypeParameterId, TypeId)],
) -> TypeId {
    substitute_impl(arena, type_id, bindings, true)
}

pub(super) fn substitute_impl(
    arena: &mut TypeArena,
    type_id: TypeId,
    bindings: &[(crate::types::TypeParameterId, TypeId)],
    keep_unbound: bool,
) -> TypeId {
    // Nothing bound, nothing to rewrite: the id comes back as it is and no scratch state
    // is built.
    if bindings.is_empty() {
        return type_id;
    }
    let mut outermost_cut = usize::MAX;
    Substitution {
        bindings,
        keep_unbound,
        preserve: Vec::new(),
        open: Vec::new(),
        rewritten: FxHashMap::default(),
        closing: FxHashMap::default(),
        cyclic: Vec::new(),
    }
    .rewrite(arena, type_id, &mut outermost_cut)
}

// Scratch state for one substitute_impl call, so that everything learned while
// rewriting one part of a type is reused by the rest of it. The graph is a DAG
// after interning, so a subtype reachable along several paths is rewritten once
// (`rewritten`) instead of once per path, and "does this contain a parameter" is
// answered from the arena's own cache (see contains_type_param) instead of a fresh
// walk at every level.
//
// `rewritten` does not outlive the call. A declaration's Ref can be resolved by
// TypeArena::resolve_ref between two calls, which would make a longer-lived rewrite
// stale.
// Maximum structural depth of one substitution walk (see `Substitution::rewrite`).
// TypeScript 6.0.3 cuts ordinary recursive type instantiation at 100. This walk's
// `open` stack is not exactly the same counter, but using the same bound prevents a
// single deeply nested generic shape from consuming an unbounded call stack.
pub(super) const MAX_INSTANTIATION_DEPTH: usize = crate::stack_guard::BUDGET.instantiation_depth;

pub(super) struct Substitution<'b> {
    bindings: &'b [(crate::types::TypeParameterId, TypeId)],
    keep_unbound: bool,
    // Parameters temporarily held back while a distributive conditional's branch
    // types are rewritten. Each union constituent gets its own replacement later.
    preserve: Vec<crate::types::TypeParameterId>,

    // Ids currently being rewritten, outermost first.
    open: Vec<TypeId>,

    rewritten: FxHashMap<TypeId, TypeId>,

    // The Ref `rewrite_recursive` has opened for an id and not yet resolved, while its
    // children are being walked. Kept apart from `rewritten` because a conditional empties
    // `rewritten` for its branches, and the edge that closes a recursive conditional alias
    // (`Flatten<T> = T extends (infer U)[] ? Flatten<U> : T`) is in a branch: with the
    // entry gone it found nothing, started the first and second pass over again, and never
    // came back.
    closing: FxHashMap<TypeId, TypeId>,

    // Ids the walk has found to reach themselves (see rewrite).
    cyclic: Vec<TypeId>,
}

impl Substitution<'_> {
    // A recursive type (Node { next: Node }, or a recursive generic like
    // `Box<T> { value: T, next: Box<T> | null }`) means substituting inside
    // type_id's properties can lead back to substituting type_id itself before the
    // first call returns. Re-entering an id already on the current path is exactly
    // that self-referential edge, and it is what stops the walk from looping.
    //
    // Leaving the edge pointing at the original id is right for a type that has
    // nothing to substitute, and wrong for one that does: `Box<number>.next` would
    // still be the generic `Box<T> | null`, and `box.next.value` would read as T.
    // So the cut only *finds* the cycle (`cyclic`); once the first pass over an
    // object has finished and it turns out to reach itself, rewrite_recursive
    // builds it again with a Ref of its own, so that the edge closes on the new object
    // instead. Only an Object can close a cycle (see TypeArena), which
    // is why nothing else gets a second pass.
    //
    // `lowest_cut` is the index in `open` of the outermost id any cut below this
    // call pointed at (usize::MAX when none did). A result is memoised only when
    // no cut reached outside the node: a rewrite that left an outer id as-is
    // reflects where the walk entered, and reusing it from another entry point
    // could differ from what a fresh walk would build there.
    fn rewrite(
        &mut self,
        arena: &mut TypeArena,
        type_id: TypeId,
        lowest_cut: &mut usize,
    ) -> TypeId {
        if let Type::GenericParameter(id, _, _) = arena.get(type_id)
            && self.preserve.contains(id)
        {
            return type_id;
        }
        if let Some(&done) = self.rewritten.get(&type_id) {
            return done;
        }
        if let Some(&copy) = self.closing.get(&type_id) {
            return copy;
        }
        // A recursive application's arguments are what a substitution changes, and the set of
        // free parameters it is cached under can be empty when the declaration it belongs to
        // closes over them (`Aw<U>` in `T extends P<infer U> ? Aw<U> : T`).
        let args_mention = matches!(arena.raw(type_id), Type::App(_, arguments, _)
            if arena.app_template(type_id).is_some()
                && arguments.iter().any(|&argument| contains_type_param(arena, argument)));
        if !args_mention && !contains_type_param(arena, type_id) {
            return type_id;
        }
        if let Some(position) = self.open.iter().position(|&id| id == type_id) {
            *lowest_cut = (*lowest_cut).min(position);
            if !self.cyclic.contains(&type_id) {
                self.cyclic.push(type_id);
            }
            return type_id;
        }

        // Problem: a type that a generic declaration builds one level at a time can be deeper
        // than anything its text shows, and substituting into it recurses once per level.
        // Picked: stop past a fixed depth, or when the stack budget is gone, and hand back the
        // error type, which does not cascade into a mismatch. The unit says so once (TSR9102).
        // A count, not only the stack, so the cut point is the same in a debug and a release
        // build and on any thread.
        let cut = if self.open.len() >= MAX_INSTANTIATION_DEPTH {
            Some(crate::stack_guard::CutReason::InstantiationDepth)
        } else if crate::stack_guard::exhausted() {
            Some(crate::stack_guard::CutReason::StackExhausted)
        } else if !crate::stack_guard::count_instantiation() {
            Some(crate::stack_guard::CutReason::InstantiationCount)
        } else {
            None
        };
        if let Some(reason) = cut {
            crate::stack_guard::note_cut(reason);
            return arena.error();
        }
        let depth = self.open.len();
        self.open.push(type_id);
        let mut cut_below = usize::MAX;
        let mut result = self.rewrite_uncached(arena, type_id, &mut cut_below);
        self.open.pop();

        if self.cyclic.contains(&type_id)
            && matches!(
                arena.get(type_id),
                Type::Object(_)
                    | Type::Array(_)
                    | Type::Function(_)
                    | Type::Union(_)
                    | Type::Intersection(_)
                    | Type::Conditional(_)
            )
        {
            // The first pass left a recursive edge on the original node, so its result
            // is discarded. Close the cycle around a Ref for every recursive structural
            // shape, not just objects: recursive conditional aliases and unions need the
            // same bounded graph rewrite as linked-list objects.
            cut_below = usize::MAX;
            result = self.rewrite_recursive(arena, type_id, &mut cut_below);
        }

        if cut_below >= depth {
            self.rewritten.insert(type_id, result);
        } else {
            // rewrite_recursive registers its Ref before descending; when
            // the result turns out not to be reusable, that entry must go too.
            self.rewritten.remove(&type_id);
        }
        *lowest_cut = (*lowest_cut).min(cut_below);
        result
    }

    // Rebuilds a recursive structural node around a Ref of its own. The Ref is
    // registered as type_id's rewrite before any child is walked, so the edge that closed
    // the first pass closes on this substituted copy. Object aliases were the first use;
    // array, union, function and conditional nodes also need this for recursive aliases.
    fn rewrite_recursive(
        &mut self,
        arena: &mut TypeArena,
        type_id: TypeId,
        cut: &mut usize,
    ) -> TypeId {
        let shape = arena.get(type_id).clone();
        if !matches!(
            &shape,
            Type::Object(_)
                | Type::Array(_)
                | Type::Function(_)
                | Type::Union(_)
                | Type::Intersection(_)
                | Type::Conditional(_)
        ) {
            return type_id;
        }

        let copy = arena.alloc_ref();
        self.rewritten.insert(type_id, copy);
        self.closing.insert(type_id, copy);
        let body = self.rewrite_shape(arena, type_id, shape, cut);
        self.closing.remove(&type_id);
        // A reducible conditional can collapse directly to its own placeholder. A Ref
        // cannot have itself as its body, so retain the symbolic conditional in that
        // exceptional self-result case rather than creating a wrapper-unwrapping loop.
        let body = if body == copy {
            match arena.get(type_id).clone() {
                Type::Conditional(conditional) => arena.alloc(Type::Conditional(conditional)),
                _ => body,
            }
        } else {
            body
        };
        if arena.slot_of(body).is_some() {
            // Never resolve one Ref to another. The placeholder is still usable as the
            // conservative result when the graph has no concrete body to close on.
            self.rewritten.insert(type_id, type_id);
            return type_id;
        }
        arena.resolve_ref(copy, body);
        copy
    }

    #[inline(never)]
    fn rewrite_indexes(
        &mut self,
        arena: &mut TypeArena,
        mut object: ObjectType,
        cut: &mut usize,
    ) -> ObjectType {
        for index in [&mut object.string_index, &mut object.number_index]
            .into_iter()
            .flatten()
        {
            index.value = self.rewrite(arena, index.value, cut);
        }
        object
    }

    fn rewrite_uncached(
        &mut self,
        arena: &mut TypeArena,
        type_id: TypeId,
        cut: &mut usize,
    ) -> TypeId {
        if let Type::App(slot, arguments, body) = arena.raw(type_id).clone() {
            let template = arena.app_template(type_id).cloned();
            let rewritten_arguments: Vec<TypeId> = arguments
                .iter()
                .map(|&argument| self.rewrite(arena, argument, cut))
                .collect();

            if let Some(template) = template {
                if rewritten_arguments != arguments {
                    // This App was first encountered while its own declaration was still
                    // resolving, so its body is a snapshot of the declaration specialized
                    // with symbolic arguments. Once an infer variable or outer generic
                    // changes those arguments, rewrite the declaration body again with the
                    // new bindings; substituting the stale body alone leaves recursive
                    // conditionals such as Flatten<T> stuck at `Flatten<infer U>`.
                    let bindings: Vec<(TypeParameterId, TypeId)> = template
                        .parameters
                        .iter()
                        .copied()
                        .zip(rewritten_arguments.iter().copied())
                        .collect();
                    // An argument that still mentions a binder this rewrite is standing under
                    // (`DP<T[K]>` inside `[K in keyof T]`) cannot be expanded yet: it is
                    // expanded when that key is replaced by a name. The application is kept
                    // with its new arguments until then.
                    if rewritten_arguments.iter().any(|&argument| {
                        free_generic_params(arena, argument)
                            .iter()
                            .any(|&(id, _)| self.preserve.contains(&id) || arena.is_mapped_key(id))
                    }) {
                        let application = arena.alloc_app(slot, rewritten_arguments, body);
                        arena.register_app_template(
                            application,
                            template.base,
                            template.parameters.clone(),
                        );
                        return application;
                    }
                    // Met again with the very bindings it is being instantiated with: the
                    // declaration reaches itself, and the application stands for that knot.
                    if arena.app_instantiation_active(template.base, &bindings) {
                        let application = arena.alloc_app(slot, rewritten_arguments, body);
                        arena.register_app_template(
                            application,
                            template.base,
                            template.parameters.clone(),
                        );
                        return application;
                    }
                    if !arena.begin_app_instantiation(template.base, &bindings) {
                        crate::stack_guard::note_cut(
                            crate::stack_guard::CutReason::RecursiveApplication,
                        );
                        return arena.error();
                    }
                    let result = substitute_bound_type_params(arena, template.base, &bindings);
                    arena.end_app_instantiation(template.base, &bindings);
                    return result;
                }

                // Unchanged arguments leave the application as it is. Its body is the
                // declaration specialized with exactly those arguments, so it has no free
                // parameter of its own to substitute, and walking it only finds the
                // declaration again through its own recursive step: the cycle that
                // `Flatten<T> = T extends (infer U)[] ? Flatten<U> : T` is made of.
                return type_id;
            }

            if rewritten_arguments != arguments
                && arena.record_value_type(type_id).is_some()
                && let [key, value] = rewritten_arguments[..]
                && let Some(record) = reduce_record(arena, key, value)
            {
                return record;
            }
            let body = self.rewrite(arena, body, cut);
            return arena.alloc_app(slot, rewritten_arguments, body);
        }

        let shape = arena.get(type_id).clone();
        self.rewrite_shape(arena, type_id, shape, cut)
    }

    fn rewrite_shape(
        &mut self,
        arena: &mut TypeArena,
        original: TypeId,
        shape: Type,
        cut: &mut usize,
    ) -> TypeId {
        match shape {
            Type::GenericParameter(id, _, _) => {
                if self.preserve.contains(&id) {
                    return original;
                }
                let bound = self
                    .bindings
                    .iter()
                    .find(|(bound, _)| *bound == id)
                    .map(|(_, resolved)| *resolved);
                match bound {
                    Some(resolved) => resolved,
                    None if self.keep_unbound => original,
                    None => arena.unknown(),
                }
            }
            Type::Array(element) => {
                let substituted = self.rewrite(arena, element, cut);
                arena.alloc(Type::Array(substituted))
            }
            Type::Tuple(elements) => {
                let mut rewritten = Vec::with_capacity(elements.len());
                for element in &elements {
                    rewritten.push(crate::types::TupleElement {
                        type_id: self.rewrite(arena, element.type_id, cut),
                        optional: element.optional,
                        rest: element.rest,
                    });
                }
                arena.alloc_tuple(rewritten)
            }
            Type::Function(function) => {
                let mut params = Vec::with_capacity(function.params.len());
                for p in &function.params {
                    params.push(crate::types::Param {
                        type_id: self.rewrite(arena, p.type_id, cut),
                        optional: p.optional,
                        rest: p.rest,
                        name: p.name.clone(),
                    });
                }
                let return_type = self.rewrite(arena, function.return_type, cut);
                let type_params = function
                    .type_params
                    .iter()
                    .map(|param| FunctionTypeParam {
                        id: param.id,
                        name: param.name.clone(),
                        constraint: param.constraint.map(|id| self.rewrite(arena, id, cut)),
                        default: param.default.map(|id| self.rewrite(arena, id, cut)),
                    })
                    .collect();
                arena.alloc(Type::Function(FunctionType {
                    params,
                    return_type,
                    type_params,
                    is_untyped: function.is_untyped,
                }))
            }
            Type::Object(object) => {
                let mut properties = Vec::with_capacity(object.properties.len());
                for p in object.properties.iter() {
                    properties.push(PropertyEntry {
                        name: p.name.clone(),
                        type_id: self.rewrite(arena, p.type_id, cut),
                        optional: p.optional,
                        is_method: p.is_method,
                        modifiers: p.modifiers.clone(),
                    });
                }
                let rebuilt = self.rewrite_indexes(arena, object.with_properties(properties), cut);
                arena.alloc(Type::Object(rebuilt))
            }
            Type::Union(members) => {
                let mut substituted = Vec::with_capacity(members.len());
                for &m in &members {
                    substituted.push(self.rewrite(arena, m, cut));
                }
                arena.alloc_union(substituted)
            }
            // Rebuilt through alloc_intersection, not allocated as it stands: replacing a
            // parameter can create something that reduces (`T & number` with `T := string`
            // is `never`), and a node that skipped the reduction would not be canonical.
            // Distributing over a union that is too large cannot be reported from here,
            // so it becomes the error type, which is already reported where it was written.
            Type::Intersection(members) => {
                let mut substituted = Vec::with_capacity(members.len());
                for &m in &members {
                    substituted.push(self.rewrite(arena, m, cut));
                }
                arena
                    .alloc_intersection(substituted)
                    .unwrap_or_else(|_| arena.error())
            }
            Type::Conditional(conditional) => {
                // An outer type argument may make this conditional decidable. Keep
                // unbound parameters inside the symbolic node until its check side
                // and extends pattern have been substituted, then reduce it once.
                let previous_keep_unbound = self.keep_unbound;
                self.keep_unbound = true;
                let check_type = self.rewrite(arena, conditional.check_type, cut);

                // The `infer` parameters are binders of this conditional: they are in
                // scope in the `extends` pattern and the true branch, and nowhere else.
                // A recursive alias (`Flatten<T> = T extends (infer Item)[] ? Flatten<Item>
                // : T`) rebuilds its body one level at a time with the same parameter ids,
                // so this conditional's check type can mention an outer `Item` while its own
                // pattern binds `Item` again. Substituting the outer `Item` into the
                // pattern as well turned `(infer Item)[]` into `number[][]` and stopped
                // the recursion one level early. They are held back here for the pattern
                // and the true branch, and not for the check type above.
                let binders_held = conditional.infer_params.len();
                self.preserve
                    .extend(conditional.infer_params.iter().copied());

                // Everything below uses its own memo: a type rewritten while the binders
                // are held, or while a distributive parameter is, is not the same rewrite
                // as one made outside. Otherwise an `extends` pattern that also contains
                // `T[]` could cache `T[] -> (string | number)[]` before the distributive
                // branch gets to preserve T, silently turning `T extends unknown ? T[] :
                // never` into an array of the entire union. Discarded afterward.
                let outer_rewrites = std::mem::take(&mut self.rewritten);
                let extends_type = self.rewrite(arena, conditional.extends_type, cut);

                if let Some(parameter) = conditional.distributive_param {
                    self.preserve.push(parameter);
                }
                let true_type = self.rewrite(arena, conditional.true_type, cut);
                // The false branch is outside the binders' scope.
                let keep = self.preserve.len()
                    - binders_held
                    - usize::from(conditional.distributive_param.is_some());
                let held: Vec<_> = self.preserve.drain(keep..).collect();
                if let Some(parameter) = conditional.distributive_param {
                    self.preserve.push(parameter);
                }
                let false_type = self.rewrite(arena, conditional.false_type, cut);
                if conditional.distributive_param.is_some() {
                    self.preserve.pop();
                }
                drop(held);
                self.rewritten = outer_rewrites;
                self.keep_unbound = previous_keep_unbound;

                resolve_conditional_type(
                    arena,
                    ConditionalType {
                        check_type,
                        extends_type,
                        true_type,
                        false_type,
                        infer_params: conditional.infer_params,
                        distributive_param: conditional.distributive_param,
                    },
                )
            }
            // `keyof T` and `T[K]` are reduced as soon as substitution leaves them nothing
            // generic; until then the rebuilt node stays symbolic.
            Type::Mapped(mapped) => {
                // A binding that mentions this very key (`DP<T[K]>` inside `DP`'s own
                // `[K in ...]`) would be captured by it, so the binder is renamed first.
                let mut mapped = mapped;
                let captured = self.bindings.iter().any(|&(_, bound)| {
                    free_generic_params(arena, bound)
                        .iter()
                        .any(|&(id, _)| id == mapped.key)
                });
                if captured
                    && let Some(&(_, node)) = free_generic_params(arena, mapped.value)
                        .iter()
                        .chain(
                            free_generic_params(arena, mapped.name_type.unwrap_or(mapped.value))
                                .iter(),
                        )
                        .find(|&&(id, _)| id == mapped.key)
                    && let Type::GenericParameter(_, name, bound) = arena.get(node).clone()
                {
                    let fresh = mapped.key.renamed();
                    arena.register_mapped_key(fresh);
                    let fresh_node = arena.alloc(Type::GenericParameter(fresh, name, bound));
                    let rename = [(mapped.key, fresh_node)];
                    mapped.value = substitute_bound_type_params(arena, mapped.value, &rename);
                    mapped.name_type = mapped
                        .name_type
                        .map(|name| substitute_bound_type_params(arena, name, &rename));
                    mapped.key = fresh;
                }
                let constraint = self.rewrite(arena, mapped.constraint, cut);
                let source = mapped.source.map(|source| self.rewrite(arena, source, cut));
                // The key is a binder of the name and the value, not of the constraint,
                // and what is rewritten under it is not the same rewrite as outside it.
                self.preserve.push(mapped.key);
                let outer_rewrites = std::mem::take(&mut self.rewritten);
                let name_type = mapped.name_type.map(|name| self.rewrite(arena, name, cut));
                let value = self.rewrite(arena, mapped.value, cut);
                self.rewritten = outer_rewrites;
                self.preserve.pop();
                reduce_operator(
                    arena,
                    Operator::Mapped(crate::types::MappedType {
                        key: mapped.key,
                        constraint,
                        source,
                        name_type,
                        value,
                        optional: mapped.optional,
                        readonly: mapped.readonly,
                    }),
                )
                .type_id()
            }
            Type::Keyof(operand) => {
                let operand = self.rewrite(arena, operand, cut);
                reduce_operator(arena, Operator::Keyof(operand)).type_id()
            }
            Type::Variadic(elements) => {
                let rewritten: Vec<crate::types::TupleElement> = elements
                    .iter()
                    .map(|element| crate::types::TupleElement {
                        type_id: self.rewrite(arena, element.type_id, cut),
                        optional: element.optional,
                        rest: element.rest,
                    })
                    .collect();
                reduce_operator(arena, Operator::Variadic(rewritten)).type_id()
            }
            Type::StringMapping(case, operand) => {
                let operand = self.rewrite(arena, operand, cut);
                reduce_operator(arena, Operator::StringMapping(case, operand)).type_id()
            }
            Type::Template(template) => {
                let types: Vec<TypeId> = template
                    .types
                    .iter()
                    .map(|&part| self.rewrite(arena, part, cut))
                    .collect();
                reduce_operator(arena, Operator::Template(template.texts, types)).type_id()
            }
            Type::IndexedAccess(object, index) => {
                let object = self.rewrite(arena, object, cut);
                let index = self.rewrite(arena, index, cut);
                reduce_operator(arena, Operator::IndexedAccess(object, index)).type_id()
            }
            _ => original,
        }
    }
}

// The free type parameters of a type: every GenericParameter reachable from it that is not
// declared by a generic function type on the way down. `<U>(x: U) => U` has none; its U is
// bound. `<U>(x: U, y: T) => U` has T. Each element is the parameter's id and the
// GenericParameter node that names it, so one walk answers three questions: is the type
// generic at all (contains_type_param), which parameters does it need explicit type
// arguments for, in declaration order (ordered_generic_param_ids), and which of them carry
// an `extends` bound (collect_generic_param_constraints).
//
// Problem 1: an earlier version of this walk carried the list of generic binders it was
// inside and keyed its "already visited" set by (type, binders). A self-referential generic
// signature never repeats that key. `class Chain<T> { then<S>(cb: (x: T) => S): Chain<S> }`
// is two types, Chain<S> and the `then` function, that lead to each other, and every lap
// through `then` pushed S onto the binder list again. gdb on the hung process showed exactly
// two type ids alternating and a binder list that went from 3,841 to 6,118 entries between
// samples: the walk never ended. On TypeScript's own tests that was 15 files that hung or
// ran out of memory (promiseChaining, promisePermutations, chainedCalls...).
// Fix: the free set of a node does not depend on the path that reached it. It is the node's
// own contents minus the binders the node itself declares, so there is no path-dependent
// key left to grow.
//
// Problem 2: the first version of that fix still walked down from the start and cut a
// cycle where it re-entered a node that was still open. An answer cut at an outer open node
// is only partial, so it was not remembered, and every other path to the same node walked
// it again. With classes that point at each other (`class C0 { p1: C1; ... p5: C5 }` and so
// on, as in resolvingClassDeclarationWhenInBaseTypeResolution) the number of paths grows
// exponentially, and the walk never finished.
// Picked: find every type reachable from the start once, then settle the free sets of all of
// them together. A set is the union of its children's sets minus the node's own binders,
// which only ever grows as the children's sets do, so repeating it until nothing changes
// reaches the one right answer for every node at once, cycles included. Each node is
// visited once to find it and a few times to settle it, so the cost follows the size of the
// graph and not the number of paths through it. The answers are exact for every node, so all
// of them are remembered in the arena, not only the ones a cut did not touch.
// Cost: one small vector per node and a few passes over the reachable graph the first time a
// type is asked about; the arena keeps the boolean, so every later question is one lookup. A
// node the arena already knows to hold no parameter is not walked into at all.
pub(super) type FreeParam = (crate::types::TypeParameterId, TypeId);

pub(super) fn is_constrained(arena: &TypeArena, node: TypeId) -> bool {
    matches!(arena.get(node), Type::GenericParameter(_, _, Some(_)))
}

// Adds `from` to `into`, one entry per parameter id, first seen first. When a parameter is
// met both as a bare node and as one carrying its constraint, the constrained node is kept:
// the constraint is the same wherever the parameter appears.
pub(super) fn merge_free(arena: &TypeArena, into: &mut Vec<FreeParam>, from: &[FreeParam]) {
    for &(id, node) in from {
        match into.iter_mut().find(|(existing, _)| *existing == id) {
            None => into.push((id, node)),
            Some(slot) => {
                if !is_constrained(arena, slot.1) && is_constrained(arena, node) {
                    slot.1 = node;
                }
            }
        }
    }
}

// The types a type is made of, in the order their free sets are merged: a generic function's
// own bounds and defaults, then its parameters, then its return type.
pub(super) fn parts_of(arena: &TypeArena, type_id: TypeId) -> SmallVec<[TypeId; 8]> {
    let mut parts = SmallVec::new();
    match arena.raw(type_id) {
        Type::App(_, arguments, body) => {
            parts.extend(arguments.iter().copied());
            parts.push(*body);
            return parts;
        }
        Type::Named(_, inner) => {
            parts.push(*inner);
            return parts;
        }
        _ => {}
    }
    match arena.get(type_id) {
        Type::Array(element) => parts.push(*element),
        Type::Tuple(elements) => parts.extend(elements.iter().map(|e| e.type_id)),
        Type::Function(function) => {
            for parameter in &function.type_params {
                parts.extend(
                    [parameter.constraint, parameter.default]
                        .into_iter()
                        .flatten(),
                );
            }
            parts.extend(function.params.iter().map(|parameter| parameter.type_id));
            parts.push(function.return_type);
        }
        Type::Object(object) => {
            parts.extend(object.properties.iter().map(|p| p.type_id));
            parts.extend(
                [&object.string_index, &object.number_index]
                    .into_iter()
                    .flatten()
                    .map(|index| index.value),
            );
        }
        Type::Union(members) | Type::Intersection(members) => parts.extend(members.iter().copied()),
        Type::Conditional(conditional) => {
            parts.extend([
                conditional.check_type,
                conditional.extends_type,
                conditional.true_type,
                conditional.false_type,
            ]);
        }
        Type::Keyof(operand) | Type::StringMapping(_, operand) => parts.push(*operand),
        Type::Variadic(elements) => parts.extend(elements.iter().map(|e| e.type_id)),
        Type::Template(template) => parts.extend(template.types.iter().copied()),
        Type::Mapped(mapped) => {
            parts.push(mapped.constraint);
            parts.extend(mapped.source);
            parts.extend(mapped.name_type);
            parts.push(mapped.value);
        }
        Type::IndexedAccess(object, index) => parts.extend([*object, *index]),
        _ => {}
    }
    parts
}

// Whether two lists name the same parameters, each by the same kind of node. The order is
// left out on purpose: it follows the order the children happened to be settled in, and it
// is the set that decides whether another pass is needed.
pub(super) fn same_free_set(arena: &TypeArena, a: &[FreeParam], b: &[FreeParam]) -> bool {
    a.len() == b.len()
        && a.iter().all(|&(id, node)| {
            b.iter().any(|&(other, other_node)| {
                other == id && is_constrained(arena, node) == is_constrained(arena, other_node)
            })
        })
}

// The free parameters of `type_id`, in first-seen order. The arena's "mentions a type
// parameter" answer is published for every node reached, so the next contains_type_param on
// any of them is one lookup.
pub(super) fn free_generic_params(arena: &TypeArena, type_id: TypeId) -> Rc<[FreeParam]> {
    if let Some(known) = arena.cached_free_params(type_id) {
        return known;
    }
    // Every type reachable from the start, each once, in the order it was found. `position`
    // doubles as the visited set.
    let mut nodes: Vec<TypeId> = Vec::new();
    let mut parts: Vec<SmallVec<[usize; 8]>> = Vec::new();
    let mut position: FxHashMap<TypeId, usize> = FxHashMap::default();
    reach(arena, type_id, &mut nodes, &mut parts, &mut position);

    // A node whose set an earlier call settled is a fixed leaf: its set already covers
    // everything reachable from it, so it is not walked or recomputed.
    let fixed: Vec<Option<Rc<[FreeParam]>>> = nodes
        .iter()
        .map(|&node| arena.cached_free_params(node))
        .collect();
    let mut sets: Vec<Vec<FreeParam>> = nodes
        .iter()
        .enumerate()
        .map(|(index, &node)| match (&fixed[index], arena.get(node)) {
            (Some(known), _) => known.to_vec(),
            (None, Type::GenericParameter(id, _, _)) => vec![(*id, node)],
            _ => Vec::new(),
        })
        .collect();

    // Last found first: a node is found before the parts it is made of, so going backwards
    // settles most parts before the node that uses them, and an acyclic graph is done in
    // one pass plus the pass that sees nothing change.
    loop {
        let mut changed = false;
        for index in (0..nodes.len()).rev() {
            if fixed[index].is_some() {
                continue;
            }
            let mut free: Vec<FreeParam> = match arena.get(nodes[index]) {
                Type::GenericParameter(id, _, _) => vec![(*id, nodes[index])],
                _ => Vec::new(),
            };
            // A recursive application (`P<T | R>` inside `P<T>`) keeps a body snapshotted while
            // its declaration was still resolving, which names the declaration's own parameter
            // although its arguments no longer do. The body stands for the arguments given, so
            // the declaration's parameters count only where an argument mentions them.
            let template_parameters: Option<Vec<TypeParameterId>> = match arena.raw(nodes[index]) {
                Type::App(..) => arena
                    .app_template(nodes[index])
                    .map(|template| template.parameters.clone()),
                _ => None,
            };
            let part_count = parts[index].len();
            for (position_in_parts, &part) in parts[index].iter().enumerate() {
                if let Some(parameters) = &template_parameters
                    && position_in_parts + 1 == part_count
                {
                    let from_body: Vec<FreeParam> = sets[part]
                        .iter()
                        .copied()
                        .filter(|(id, _)| !parameters.contains(id))
                        .collect();
                    merge_free(arena, &mut free, &from_body);
                    continue;
                }
                merge_free(arena, &mut free, &sets[part]);
            }
            // The function's own binders are bound inside it, wherever they were used.
            if let Type::Function(function) = arena.get(nodes[index]) {
                free.retain(|(id, _)| !function.type_params.iter().any(|own| own.id == *id));
            }
            if let Type::Conditional(conditional) = arena.get(nodes[index]) {
                free.retain(|(id, _)| !conditional.infer_params.contains(id));
            }
            // The key of a mapped type is bound inside it.
            if let Type::Mapped(mapped) = arena.get(nodes[index]) {
                free.retain(|(id, _)| *id != mapped.key);
            }
            if !same_free_set(arena, &free, &sets[index]) {
                changed = true;
            }
            sets[index] = free;
        }
        if !changed {
            break;
        }
    }

    for (index, (node, set)) in nodes.iter().zip(&sets).enumerate() {
        arena.remember_mentions_type_param(*node, !set.is_empty());
        if !set.is_empty() && fixed[index].is_none() {
            arena.remember_free_params(*node, Rc::from(set.clone()));
        }
    }
    Rc::from(sets[position[&type_id]].clone())
}

// Finds `type_id` and everything reachable from it, once each. A type the arena already
// knows to mention no parameter is a leaf: it can only contribute an empty set.
pub(super) fn reach(
    arena: &TypeArena,
    type_id: TypeId,
    nodes: &mut Vec<TypeId>,
    parts: &mut Vec<SmallVec<[usize; 8]>>,
    position: &mut FxHashMap<TypeId, usize>,
) -> usize {
    if let Some(&known) = position.get(&type_id) {
        return known;
    }
    // Reserved before the parts are visited, so a cycle back to this node finds its slot.
    let slot = nodes.len();
    position.insert(type_id, slot);
    nodes.push(type_id);
    parts.push(SmallVec::new());
    if arena.cached_mentions_type_param(type_id) != Some(false)
        && arena.cached_free_params(type_id).is_none()
    {
        let children: SmallVec<[usize; 8]> = parts_of(arena, type_id)
            .into_iter()
            .map(|child| reach(arena, child, nodes, parts, position))
            .collect();
        parts[slot] = children;
    }
    slot
}

// Whether a type mentions a *free* type parameter, i.e. one not declared by a generic
// function type inside it. Answers are kept in the arena (TypeArena::cached_mentions_type_param),
// not per call: the checker asks it for every call expression, mostly about types that never
// change, and interning has turned the type graph into a DAG.
pub fn contains_type_param(arena: &TypeArena, type_id: TypeId) -> bool {
    if let Some(known) = arena.cached_mentions_type_param(type_id) {
        return known;
    }
    !free_generic_params(arena, type_id).is_empty()
}

/// Whether a type mentions a free type parameter that is one of `ids`.
pub fn mentions_any_param(
    arena: &TypeArena,
    type_id: TypeId,
    ids: &[crate::types::TypeParameterId],
) -> bool {
    contains_type_param(arena, type_id)
        && free_generic_params(arena, type_id)
            .iter()
            .any(|&(id, _)| ids.contains(&id))
}
