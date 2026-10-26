// Strongly connected components and layers over plain adjacency lists.
//
// Problem: the module graph and the petgraph view of it each had their own component
// search, kept equal by a test, and neither could be tested on graphs other than the few
// built by hand.
// Picked: one iterative Tarjan over `&[Vec<usize>]` that both call, plus the layering rule
// beside it, so a test can feed both random graphs and compare against a brute-force answer.
// Cost: callers copy their edges into index lists once per build.
// Now: an import chain is as deep as the project's longest dependency path, and recursion
// would put that on the thread's call stack, where a long enough chain overflows it, so the
// search keeps its own stack.

const UNVISITED: u32 = u32::MAX;

/// Strongly connected components of the graph whose node `n` depends on `dependencies[n]`.
/// Members of a component are sorted. Components come out dependencies first: one is
/// emitted only after every component it depends on.
pub(crate) fn components(dependencies: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let count = dependencies.len();
    let mut order = vec![UNVISITED; count];
    let mut low_link = vec![0u32; count];
    let mut on_stack = vec![false; count];
    let mut stack: Vec<usize> = Vec::new();
    let mut found = Vec::new();
    let mut next_order = 0u32;
    // Each frame is a node and how many of its dependencies have been looked at.
    let mut frames: Vec<(usize, usize)> = Vec::new();

    for root in 0..count {
        if order[root] != UNVISITED {
            continue;
        }
        order[root] = next_order;
        low_link[root] = next_order;
        next_order += 1;
        stack.push(root);
        on_stack[root] = true;
        frames.push((root, 0));

        while let Some(frame) = frames.last_mut() {
            let node = frame.0;
            if frame.1 < dependencies[node].len() {
                let next = dependencies[node][frame.1];
                frame.1 += 1;
                if order[next] == UNVISITED {
                    order[next] = next_order;
                    low_link[next] = next_order;
                    next_order += 1;
                    stack.push(next);
                    on_stack[next] = true;
                    frames.push((next, 0));
                } else if on_stack[next] {
                    low_link[node] = low_link[node].min(order[next]);
                }
                continue;
            }

            frames.pop();
            if let Some(parent) = frames.last() {
                let parent = parent.0;
                low_link[parent] = low_link[parent].min(low_link[node]);
            }
            if low_link[node] == order[node] {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack[member] = false;
                    component.push(member);
                    if member == node {
                        break;
                    }
                }
                component.sort_unstable();
                found.push(component);
            }
        }
    }
    found
}

/// For each node, the index of the component that holds it.
pub(crate) fn component_of(count: usize, components: &[Vec<usize>]) -> Vec<u32> {
    let mut of = vec![0u32; count];
    for (position, component) in components.iter().enumerate() {
        for member in component {
            of[*member] = u32::try_from(position).expect("fewer than 2^32 components");
        }
    }
    of
}

/// Groups the nodes so that everything in a layer depends only on earlier layers, except
/// for dependencies among members of one component, which share a layer. A layer's depth
/// is the length of the longest chain of components below it, so no layer is later than it
/// has to be. Each layer is sorted. `components` must be in the order `components` returns.
pub(crate) fn layers(dependencies: &[Vec<usize>], components: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let of = component_of(dependencies.len(), components);
    // `components` lists a component after every component it depends on, so each depth
    // is final by the time something depends on it.
    let mut depth: Vec<usize> = vec![0; components.len()];
    let mut layers: Vec<Vec<usize>> = Vec::new();
    for (position, component) in components.iter().enumerate() {
        let mut level = 0usize;
        for member in component {
            for dependency in &dependencies[*member] {
                let other = of[*dependency] as usize;
                if other != position {
                    level = level.max(depth[other] + 1);
                }
            }
        }
        depth[position] = level;
        if layers.len() <= level {
            layers.resize_with(level + 1, Vec::new);
        }
        layers[level].extend(component.iter().copied());
    }
    for layer in &mut layers {
        layer.sort_unstable();
    }
    layers
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{component_of, components, layers};

    // A small deterministic generator, so a failing case can be replayed from its seed
    // and the test needs no new dependency.
    pub(crate) struct Xorshift(pub(crate) u64);

    impl Xorshift {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        pub(crate) fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    // `reach[a][b]`: b can be reached from a by one or more dependency edges.
    fn reachability(dependencies: &[Vec<usize>]) -> Vec<Vec<bool>> {
        let count = dependencies.len();
        let mut reach = vec![vec![false; count]; count];
        for (from, targets) in dependencies.iter().enumerate() {
            for to in targets {
                reach[from][*to] = true;
            }
        }
        for via in 0..count {
            for from in 0..count {
                if reach[from][via] {
                    for to in 0..count {
                        if reach[via][to] {
                            reach[from][to] = true;
                        }
                    }
                }
            }
        }
        reach
    }

    pub(crate) fn random_graph(rng: &mut Xorshift) -> Vec<Vec<usize>> {
        let count = 1 + rng.below(40);
        // Sparse graphs have long chains and few cycles, dense ones the opposite.
        let density = 1 + rng.below(6);
        let mut dependencies = vec![Vec::new(); count];
        for from in 0..count {
            for to in 0..count {
                if rng.below(count * 3 / density + 1) == 0 {
                    dependencies[from].push(to);
                }
            }
            // Edge lists in the real graph are sorted and unique; shuffle the order of
            // discovery by reversing half of them, the result must not depend on it.
            if rng.below(2) == 0 {
                dependencies[from].reverse();
            }
        }
        dependencies
    }

    #[test]
    fn components_match_mutual_reachability_on_random_graphs() {
        for seed in 1..=300u64 {
            let mut rng = Xorshift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let dependencies = random_graph(&mut rng);
            let found = components(&dependencies);
            let reach = reachability(&dependencies);
            let of = component_of(dependencies.len(), &found);

            // Every node is in exactly one component, and members are sorted.
            let mut seen = vec![0usize; dependencies.len()];
            for component in &found {
                assert!(
                    component.windows(2).all(|pair| pair[0] < pair[1]),
                    "seed {seed}"
                );
                for member in component {
                    seen[*member] += 1;
                }
            }
            assert!(
                seen.iter().all(|times| *times == 1),
                "seed {seed}: {found:?}"
            );

            // Two nodes share a component exactly when each can reach the other.
            for a in 0..dependencies.len() {
                for b in 0..dependencies.len() {
                    let mutual = a == b || (reach[a][b] && reach[b][a]);
                    assert_eq!(of[a] == of[b], mutual, "seed {seed}: nodes {a} and {b}");
                }
            }

            // Dependencies first: a component is never emitted before one it depends on.
            for (from, targets) in dependencies.iter().enumerate() {
                for to in targets {
                    assert!(of[*to] <= of[from], "seed {seed}: edge {from} -> {to}");
                }
            }
        }
    }

    #[test]
    fn layers_are_the_longest_chain_below_each_component_on_random_graphs() {
        for seed in 1..=300u64 {
            let mut rng = Xorshift(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
            let dependencies = random_graph(&mut rng);
            let found = components(&dependencies);
            let of = component_of(dependencies.len(), &found);
            let grouped = layers(&dependencies, &found);

            let mut layer_of = vec![usize::MAX; dependencies.len()];
            for (index, layer) in grouped.iter().enumerate() {
                assert!(
                    layer.windows(2).all(|pair| pair[0] < pair[1]),
                    "seed {seed}"
                );
                assert!(!layer.is_empty(), "seed {seed}: empty layer {index}");
                for member in layer {
                    assert_eq!(layer_of[*member], usize::MAX, "seed {seed}: listed twice");
                    layer_of[*member] = index;
                }
            }
            assert!(
                layer_of.iter().all(|index| *index != usize::MAX),
                "seed {seed}"
            );

            // Members of one component share a layer.
            for component in &found {
                let first = layer_of[component[0]];
                assert!(
                    component.iter().all(|m| layer_of[*m] == first),
                    "seed {seed}"
                );
            }

            // Brute force: the depth of a component is one more than the deepest
            // component it depends on, computed by repeating until nothing changes.
            let mut depth = vec![0usize; found.len()];
            loop {
                let mut changed = false;
                for (from, targets) in dependencies.iter().enumerate() {
                    for to in targets {
                        if of[*to] != of[from]
                            && depth[of[from] as usize] < depth[of[*to] as usize] + 1
                        {
                            depth[of[from] as usize] = depth[of[*to] as usize] + 1;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
            for node in 0..dependencies.len() {
                assert_eq!(
                    layer_of[node], depth[of[node] as usize],
                    "seed {seed}: node {node}"
                );
            }
        }
    }

    #[test]
    fn a_long_chain_does_not_overflow_the_stack() {
        // 200,000 files in one import chain: a recursive search would need that many frames.
        let count = 200_000;
        let dependencies: Vec<Vec<usize>> = (0..count)
            .map(|node| {
                if node + 1 < count {
                    vec![node + 1]
                } else {
                    Vec::new()
                }
            })
            .collect();
        let found = components(&dependencies);
        assert_eq!(found.len(), count);
        assert_eq!(found[0], vec![count - 1]);
        assert_eq!(layers(&dependencies, &found).len(), count);
    }

    #[test]
    fn a_self_import_is_one_component_of_one() {
        let dependencies = vec![vec![0], vec![0]];
        assert_eq!(components(&dependencies), vec![vec![0], vec![1]]);
    }
}
