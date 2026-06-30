//! Compile-time relation closures (symmetric / reflexive / equivalence / SCC /
//! transitive) over a relation`s `(from, to)` pairs.
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Every node a relation's pairs mention (subjects and objects), deduped/sorted.
pub(crate) fn relation_nodes(pairs: &[(String, String)]) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    for (a, b) in pairs {
        s.insert(a.clone());
        s.insert(b.clone());
    }
    s
}

/// Symmetric closure: add `(b, a)` for every `(a, b)`. O(E). Compile-time.
pub(crate) fn symmetric_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut set: BTreeSet<(String, String)> = pairs.into_iter().collect();
    let backs: Vec<(String, String)> = set.iter().map(|(a, b)| (b.clone(), a.clone())).collect();
    set.extend(backs);
    set.into_iter().collect()
}

/// Reflexive closure: add `(x, x)` for every node the relation mentions. O(V).
pub(crate) fn reflexive_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let nodes = relation_nodes(&pairs);
    let mut set: BTreeSet<(String, String)> = pairs.into_iter().collect();
    for n in nodes {
        set.insert((n.clone(), n));
    }
    set.into_iter().collect()
}

/// Equivalence closure: reflexive + symmetric + transitive — groups nodes into
/// classes (`a ~ b` iff connected ignoring direction). O(V³) via the transitive
/// step. Cycles are expected here, so (unlike `transitive_closure`) no error.
pub(crate) fn equivalence_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    reflexive_closure(transitive_closure(symmetric_closure(pairs)))
}

/// Strongly-connected grouping: keep `(a, b)` where `a` and `b` reach each other
/// (mutual reachability), plus each node to itself. Isolates directed cycles.
/// O(V³) for the reachability + O(V²) for the mutual filter. Compile-time.
pub(crate) fn scc_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let nodes = relation_nodes(&pairs);
    let reach: BTreeSet<(String, String)> = transitive_closure(pairs).into_iter().collect();
    let mut out: BTreeSet<(String, String)> = BTreeSet::new();
    for (a, b) in &reach {
        if reach.contains(&(b.clone(), a.clone())) {
            out.insert((a.clone(), b.clone()));
        }
    }
    // Each node is its own (possibly singleton) component.
    for n in nodes {
        out.insert((n.clone(), n));
    }
    out.into_iter().collect()
}

/// The transitive closure of a relation's `(from, to)` pairs: add `(a, c)`
/// whenever `(a, b)` and `(b, c)` are present, to a fixpoint. A self-pair
/// `(x, x)` in the result marks a cycle. A small compile-time graph op.
pub(crate) fn transitive_closure(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    // Index the relation as an adjacency map (each node to its direct
    // successors), then collect everything reachable from each start node along
    // a path of length >= 1. This is the same relation the naive pairwise
    // fixpoint produced, but found in one reachability sweep per node instead of
    // rescanning every pair against every pair until saturation.
    let mut succ: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (a, b) in &pairs {
        succ.entry(a).or_default().push(b);
    }
    let mut out: BTreeSet<(String, String)> = BTreeSet::new();
    for (&start, direct) in &succ {
        let mut stack: Vec<&str> = direct.clone();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(node) = stack.pop() {
            if seen.insert(node) {
                out.insert((start.to_string(), node.to_string()));
                if let Some(next) = succ.get(node) {
                    stack.extend(next.iter().copied());
                }
            }
        }
    }
    out.into_iter().collect()
}
