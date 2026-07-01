//! Post-model diagnostics that never change the verdict: orphan facts and the
//! "did you mean" near-miss atom pairs.
use crate::report::{OrphanFact, SimilarAtoms, label};
use crate::unsat::key;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use elenchus_compiler::{AtomId, AtomKey, Compiled, levenshtein};

/// Collect the **orphan facts**: every `FACT`/`NOT`/`ASSUME` whose atom appears
/// in no `PREMISE` clause and no `RULE`. Such an assertion is inert — it cannot be
/// checked and derives nothing, so it has no bearing on the verdict.
///
/// An atom is "referenced" if it occurs in any desugared `PREMISE` clause
/// (`c.clauses`) or in any `RULE`'s antecedent or consequent. (The built-in
/// non-contradiction is not a clause here — it is enforced during fact seeding —
/// so it does not mask an orphan.) Deterministic: facts keep source order; the
/// result is sorted by origin (source, line).
pub(crate) fn orphan_facts(c: &Compiled) -> Vec<OrphanFact> {
    let mut referenced = vec![false; c.atoms.len()];
    for clause in &c.clauses {
        for l in &clause.lits {
            referenced[l.atom as usize] = true;
        }
    }
    for r in &c.rules {
        for l in r
            .antecedent
            .iter()
            .chain(r.consequent.iter())
            .chain(r.exceptions.iter())
        {
            referenced[l.atom as usize] = true;
        }
    }
    // Edges consumed by a relation `FOR EACH` are read as data, not idle facts.
    for &a in &c.consumed {
        referenced[a as usize] = true;
    }
    // A `FACT … BECAUSE <ground>` reads its ground (the justification check) and its
    // belief carries an explicit justification — neither is an inert leftover.
    for j in &c.justifications {
        referenced[j.belief as usize] = true;
        referenced[j.ground as usize] = true;
    }
    let mut out: Vec<OrphanFact> = c
        .facts
        .iter()
        .filter(|f| !referenced[f.atom as usize])
        .map(|f| OrphanFact {
            atom: label(c, f.atom),
            value: f.value,
            origin: f.origin.clone(),
        })
        .collect();
    out.sort_by_key(|o| key(&o.origin));
    out
}

/// Detect pairs of distinct atoms whose names look like the same atom typed two
/// ways. Two deliberately conservative signals (keep false positives minimal):
///
/// - **A — fold-equal:** identical after lowercasing and treating `_`/whitespace
///   as one separator (`Has_fuel`/`has_fuel`, `is_rolled_back`/`is rolled_back`).
///   Distinct atoms that fold to the same string are almost always one typo.
/// - **B — near edit:** *same subject*, an *alphabetic (cased)* script, and a
///   Levenshtein distance of exactly 1 over the folded form (names ≥ 5 chars).
///   Distance 1 only — distance 2 flags real antonyms (mortal/immortal) far too
///   often. Edit distance is a typo signal only where a word spans many
///   characters; in caseless scripts (CJK / kana / hangul) one character is a
///   whole word, so a one-character change is normally a *different* word — those
///   are skipped by the "cased letters only" test (no hard-coded Unicode ranges).
///
/// Signal A is fully script-agnostic; signal B is the script-sensitive one.
/// `O(n²)` over the (typically small) atom set, with a length-difference quick
/// reject. Deterministic: atoms are already canonically sorted in `Compiled`.
pub(crate) fn similar_atom_pairs(c: &Compiled) -> Vec<SimilarAtoms> {
    let folded: Vec<Vec<char>> = c.atoms.iter().map(fold_atom).collect();
    let cased: Vec<bool> = folded.iter().map(|f| is_cased_alphabetic(f)).collect();
    // Edge atoms consumed by a relation FOR EACH (e.g. `a linked b`, `a linked c`)
    // legitimately differ by one character — never flag them as look-alike typos.
    let mut consumed = vec![false; c.atoms.len()];
    for &a in &c.consumed {
        consumed[a as usize] = true;
    }
    let mut out = Vec::new();
    for i in 0..c.atoms.len() {
        if consumed[i] {
            continue;
        }
        for j in (i + 1)..c.atoms.len() {
            if consumed[j] {
                continue;
            }
            if let Some(reason) = atoms_look_similar(
                &c.atoms[i],
                &folded[i],
                cased[i],
                &c.atoms[j],
                &folded[j],
                cased[j],
            ) {
                out.push(SimilarAtoms {
                    a: label(c, i as AtomId),
                    b: label(c, j as AtomId),
                    reason,
                });
            }
        }
    }
    out
}

/// Fold an atom to its comparison form: `subject predicate [object]` lowercased,
/// every `_`/whitespace run collapsed to a single space. So `_` vs space vs case
/// can never distinguish two names.
pub(crate) fn fold_atom(k: &AtomKey) -> Vec<char> {
    let mut raw = String::new();
    raw.push_str(&k.subject);
    if let Some(p) = &k.predicate {
        raw.push(' ');
        raw.push_str(p);
    }
    if let Some(o) = &k.object {
        raw.push(' ');
        raw.push_str(o);
    }
    let mut out: Vec<char> = Vec::new();
    let mut prev_space = false;
    for ch in raw.chars() {
        if ch == '_' || ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        }
    }
    if out.last() == Some(&' ') {
        out.pop();
    }
    out
}

/// Whether every character of a folded name is a space or a *cased* letter — the
/// script-agnostic gate for edit-distance (signal B). Cased scripts (Latin,
/// Cyrillic, Greek, …) span many characters per word, so a one-character edit is
/// a plausible typo. Caseless scripts (CJK / kana / hangul, where one character
/// is a whole word) and digits report `is_lowercase() == false` after folding, so
/// they fall out here without enumerating any Unicode ranges.
pub(crate) fn is_cased_alphabetic(folded: &[char]) -> bool {
    folded.iter().all(|&c| c == ' ' || c.is_lowercase())
}

/// The two-signal similarity test (see [`similar_atom_pairs`]). Returns the
/// reason string when the pair looks like a typo, else `None`.
pub(crate) fn atoms_look_similar(
    ka: &AtomKey,
    fa: &[char],
    cased_a: bool,
    kb: &AtomKey,
    fb: &[char],
    cased_b: bool,
) -> Option<&'static str> {
    // A — same folded form in the SAME domain (the AtomKeys differ, so the raw
    // spelling differs). Atoms in different domains are legitimately distinct
    // even when their triples fold equal, so they are never flagged.
    if fa == fb && ka.domain == kb.domain {
        return Some("same name up to case, '_', or spaces");
    }
    // B — same subject, an alphabetic (cased) script, a single-character slip.
    // Only distance 1: distance 2 flags real antonyms (mortal/immortal) and word
    // pairs far too often — genuine typos are almost always a one-character edit,
    // and the underscore/case case is already covered by signal A.
    if !cased_a || !cased_b || ka.domain != kb.domain || ka.subject != kb.subject {
        return None;
    }
    if fa.len().abs_diff(fb.len()) > 1 {
        return None; // edit distance >= length difference, so it can't be 1
    }
    let min_len = fa.len().min(fb.len());
    if min_len >= 5 && levenshtein(fa, fb) == 1 {
        return Some("looks like a one-character typo of each other");
    }
    None
}
