use crate::arena::{TypeArena, TypeId};
use crate::fxhash::FxHashMap;
use crate::subtyping;

/// Semantic operations exposed to AST-facing checker code.
///
/// The query layer is intentionally small. It gives bridge features a stable
/// place to ask semantic questions without making the bridge depend on the
/// implementation details of individual type-system algorithms.
///
/// Despite taking `&mut` (for the cache below), this still asks nothing of
/// the type system that a read-only view couldn't answer; it does not
/// allocate types or otherwise mutate the arena. The `&mut` is purely to let
/// repeated identical questions be answered from a cache instead of
/// re-walking the type structure.
// One cache serves every relation, with the relation in the key, so adding
// is_disjoint did not mean threading a second map through every place that already
// carries this one. Generic inference cannot hold a SemanticQueries across its arena
// mutations, so it carries the cache by hand and spells the type the same way the
// owner does.
//
// The cache is scoped to one arena generation. Finishing a recursive declaration
// (TypeArena::set) or starting a new file (TypeArena::clear) changes what an existing
// TypeId means, and an answer computed against the old meaning must not survive. The
// generation check makes that automatic: the next query through SemanticQueries::new
// drops every entry from an older generation, so no caller has to remember a rule.
#[derive(Default)]
pub struct RelationCache {
    generation: u64,
    values: FxHashMap<(Relation, TypeId, TypeId), bool>,
    hits: u64,
    misses: u64,
}

/// Cache counters for benchmarks and regression reports. Hits and misses are
/// cumulative over the cache's life, including across generation resets, so a report
/// shows total reuse and not just reuse since the last reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueryStats {
    pub relation_hits: u64,
    pub relation_misses: u64,
    pub relation_entries: usize,
}

impl RelationCache {
    fn prepare(&mut self, generation: u64) {
        if self.generation != generation {
            self.values.clear();
            self.generation = generation;
        }
    }

    fn get(&mut self, key: &(Relation, TypeId, TypeId)) -> Option<bool> {
        let found = self.values.get(key).copied();
        if found.is_some() {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
        found
    }

    fn insert(&mut self, key: (Relation, TypeId, TypeId), value: bool) {
        self.values.insert(key, value);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn stats(&self) -> QueryStats {
        QueryStats {
            relation_hits: self.hits,
            relation_misses: self.misses,
            relation_entries: self.values.len(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Relation {
    Subtype,
    Disjoint,
}

pub struct SemanticQueries<'a> {
    arena: &'a TypeArena,

    // Memoizes relation answers by relation and TypeId pair. See the doc comment
    // on CheckContext::relation_cache, which owns this map for the lifetime of
    // one file's check, for what this does and does not cover.
    cache: &'a mut RelationCache,
}

impl<'a> SemanticQueries<'a> {
    pub(crate) fn new(arena: &'a TypeArena, cache: &'a mut RelationCache) -> Self {
        cache.prepare(arena.generation());
        Self { arena, cache }
    }

    /// Whether a value of type `source` can be used where `target` is
    /// expected. Currently identical to `is_subtype`: TypeScript's real
    /// assignability rules (excess property checks, `const` assertions,
    /// etc.) diverge from plain subtyping in cases tsox doesn't
    /// implement yet. `is_assignable` is kept as its own method so bridge
    /// code calls the concept it actually means, and so that divergence
    /// can be added here later without touching any call site.
    pub fn is_assignable(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_subtype(source, target)
    }

    // Every result gets cached unconditionally, including a single point lookup
    // that would never be asked again: checking for reuse potential first
    // would cost more than the memoization ever saves. This is a leaf method:
    // it is the only thing in the checker allowed to call subtyping::is_subtype
    // directly, precisely so "did this go through the cache" has one place to
    // check rather than needing an audit of every call site. The one deliberate
    // exception is type_annotation::check_type_argument_constraint: it runs while
    // a declaration is still being resolved, when its Ref reads as an empty object, and
    // a result cached against that empty object would outlive it. Generic
    // inference is not an exception: it runs after every declaration is complete
    // and goes through here.
    pub fn is_subtype(&mut self, source: TypeId, target: TypeId) -> bool {
        // Answered before the map is touched. Recomputing these costs a couple of
        // integer compares, less than hashing a (TypeId, TypeId) key, and storing
        // them would only grow the map with entries that can never save any work.
        // Error and Any operands are the common case once one diagnostic has been
        // reported, since Error then flows through every expression built on it,
        // so this is where a broken file would otherwise flood the cache.
        if subtyping::is_trivial_subtype(self.arena, source, target) {
            return true;
        }

        let key = (Relation::Subtype, source, target);
        if let Some(cached) = self.cache.get(&key) {
            return cached;
        }

        let result = subtyping::is_subtype(self.arena, source, target);
        self.cache.insert(key, result);
        result
    }

    /// Whether no value can belong to both types. True only when that is certain; see
    /// subtyping::is_disjoint for why an undecidable pair answers false.
    ///
    /// Symmetric, so the key stores the two ids in a fixed order and (a, b) shares
    /// its entry with (b, a). Nothing calls this yet outside tests; it is the
    /// relation switch/if narrowing will use next to drop union members that
    /// cannot equal a literal, so the allow keeps the build quiet until then.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_disjoint(&mut self, a: TypeId, b: TypeId) -> bool {
        let (first, second) = if a.index() <= b.index() {
            (a, b)
        } else {
            (b, a)
        };
        let key = (Relation::Disjoint, first, second);
        if let Some(cached) = self.cache.get(&key) {
            return cached;
        }

        let result = subtyping::is_disjoint(self.arena, a, b);
        self.cache.insert(key, result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{RelationCache, SemanticQueries};
    use crate::arena::TypeArena;
    use crate::subtyping;

    #[test]
    fn assignability_delegates_to_the_central_type_relation() {
        let arena = TypeArena::new();
        let mut cache = RelationCache::default();
        let mut queries = SemanticQueries::new(&arena, &mut cache);

        assert!(queries.is_assignable(arena.string(), arena.unknown()));
        assert!(!queries.is_assignable(arena.number(), arena.string()));
    }

    #[test]
    fn disjointness_is_symmetric_and_shares_one_cache_entry() {
        let arena = TypeArena::new();
        let mut cache = RelationCache::default();
        let mut queries = SemanticQueries::new(&arena, &mut cache);

        assert!(queries.is_disjoint(arena.string(), arena.number()));
        assert!(queries.is_disjoint(arena.number(), arena.string()));

        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn disjointness_and_subtyping_do_not_share_answers() {
        let arena = TypeArena::new();
        let mut cache = RelationCache::default();
        let mut queries = SemanticQueries::new(&arena, &mut cache);

        assert!(!queries.is_subtype(arena.number(), arena.string()));
        assert!(queries.is_disjoint(arena.number(), arena.string()));
        assert!(!queries.is_subtype(arena.number(), arena.string()));

        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn cached_result_matches_the_uncached_answer() {
        let arena = TypeArena::new();
        let mut cache = RelationCache::default();
        let mut queries = SemanticQueries::new(&arena, &mut cache);

        // First call computes and caches; second call must hit the cache and
        // still return the same answer, not a stale or default value.
        assert!(queries.is_subtype(arena.number(), arena.unknown()));
        assert!(queries.is_subtype(arena.number(), arena.unknown()));

        assert!(!queries.is_subtype(arena.boolean(), arena.string()));
        assert!(!queries.is_subtype(arena.boolean(), arena.string()));
    }

    #[test]
    fn trivial_pairs_are_answered_without_a_cache_entry() {
        // Guards the reason the fast path exists: if these ever start landing in the
        // map again, the bypass has silently stopped doing its job, and nothing else
        // would notice because the answers stay correct either way.
        let arena = TypeArena::new();
        let mut cache = RelationCache::default();
        let mut queries = SemanticQueries::new(&arena, &mut cache);

        assert!(queries.is_subtype(arena.number(), arena.number()));
        assert!(queries.is_subtype(arena.error(), arena.string()));
        assert!(queries.is_subtype(arena.string(), arena.any()));
        assert!(cache.is_empty(), "trivial pairs must not be memoized");
    }

    #[test]
    fn resolving_a_declaration_discards_answers_computed_against_the_empty_one() {
        use crate::types::{ObjectType, PropertyEntry, Type};

        let mut arena = TypeArena::new();
        let declaration = arena.alloc_ref();
        let shape = |arena: &TypeArena| {
            Type::Object(ObjectType::new(vec![PropertyEntry {
                name: "value".into(),
                type_id: arena.number(),
                optional: false,
                is_method: false,
            }]))
        };
        let target = {
            let ty = shape(&arena);
            arena.alloc(ty)
        };
        let mut cache = RelationCache::default();

        {
            let mut queries = SemanticQueries::new(&arena, &mut cache);
            assert!(!queries.is_subtype(declaration, target));
        }
        assert_eq!(cache.len(), 1);

        let filled = shape(&arena);
        let body = arena.alloc(filled);
        arena.resolve_ref(declaration, body);

        let mut queries = SemanticQueries::new(&arena, &mut cache);
        assert!(
            queries.is_subtype(declaration, target),
            "the answer from the empty declaration must not survive resolve_ref()"
        );
        assert_eq!(cache.len(), 1, "the old generation's entry was dropped");
    }

    #[test]
    fn stats_count_hits_and_misses_across_a_generation_reset() {
        let mut arena = TypeArena::new();
        let mut cache = RelationCache::default();
        let number = arena.number();
        let string = arena.string();

        {
            let mut queries = SemanticQueries::new(&arena, &mut cache);
            assert!(!queries.is_subtype(number, string));
            assert!(!queries.is_subtype(number, string));
        }
        let before = cache.stats();
        assert_eq!((before.relation_misses, before.relation_hits), (1, 1));

        arena.clear();
        let mut queries = SemanticQueries::new(&arena, &mut cache);
        assert!(!queries.is_subtype(number, string));
        let after = cache.stats();
        assert_eq!(after.relation_misses, 2, "counters keep accumulating");
        assert_eq!(after.relation_entries, 1);
    }

    #[test]
    fn non_trivial_pairs_still_populate_the_cache() {
        // The other half of the guard above: the bypass must stay narrow, otherwise
        // the cache would quietly stop caching the questions it exists for.
        let arena = TypeArena::new();
        let mut cache = RelationCache::default();
        let mut queries = SemanticQueries::new(&arena, &mut cache);

        assert!(!queries.is_subtype(arena.number(), arena.string()));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_does_not_populate_the_reverse_pair() {
        // Subtyping is not symmetric (see subtyping.rs: literal -> primitive
        // widening only goes one way). A cache keyed without direction would
        // silently make is_subtype(b, a) return whatever is_subtype(a, b)
        // returned. This must not happen.
        let mut arena = TypeArena::new();
        let mut cache = RelationCache::default();

        let five = arena.alloc(crate::types::Type::NumberLiteral(5.0));
        let number = arena.number();

        let mut queries = SemanticQueries::new(&arena, &mut cache);

        assert!(queries.is_subtype(five, number), "5 is a subtype of number");
        assert!(
            !queries.is_subtype(number, five),
            "number is not a subtype of the literal 5, even after caching the reverse pair"
        );
    }

    #[test]
    fn repeated_calls_across_many_pairs_stay_correct() {
        // Stress the cache with many distinct pairs, interleaved with repeats
        // of earlier pairs, checking every answer against a direct
        // (uncached) call every time. This is the property a cache must
        // never violate: memoizing must never change an answer, no matter
        // how many other entries have been inserted since, or in what order
        // pairs are revisited.
        let mut arena = TypeArena::new();
        let mut cache = RelationCache::default();

        let mut ids = vec![
            arena.number(),
            arena.string(),
            arena.boolean(),
            arena.unknown(),
            arena.any(),
            arena.never(),
        ];
        for i in 0..20 {
            ids.push(arena.alloc(crate::types::Type::NumberLiteral(i as f64)));
        }
        for i in 0..20 {
            ids.push(arena.alloc(crate::types::Type::StringLiteral(format!("s{i}"))));
        }

        let mut queries = SemanticQueries::new(&arena, &mut cache);

        // Round 1: every ordered pair, populating the cache.
        for &a in &ids {
            for &b in &ids {
                let cached = queries.is_subtype(a, b);
                let direct = subtyping::is_subtype(&arena, a, b);
                assert_eq!(cached, direct, "mismatch on first visit to ({a:?}, {b:?})");
            }
        }

        // Round 2: same pairs again, so every one is a cache hit.
        for &a in &ids {
            for &b in &ids {
                let cached = queries.is_subtype(a, b);
                let direct = subtyping::is_subtype(&arena, a, b);
                assert_eq!(
                    cached, direct,
                    "mismatch on cached revisit to ({a:?}, {b:?})"
                );
            }
        }
    }
}
