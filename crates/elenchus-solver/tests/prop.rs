//! Property-based tests (proptest).
//!
//! The SAT core is cross-checked against an exhaustive brute-force oracle over
//! small random formulas: any disagreement on SAT/UNSAT, an invalid model, or a
//! wrong model count is a real bug. This is the cheapest strong evidence that the
//! CDCL implementation (watched literals, 1-UIP learning, backjumping) is correct
//! — much cheaper than DRAT proof checking and sufficient at our scale.

use elenchus_compiler::{AtomId, AtomKey, Clause, Compiled, Fact, Lit, Origin, Value};
use elenchus_solver::sat::{self, Cnf, SatLit, Var};
use elenchus_solver::{Status, solve};
use proptest::prelude::*;

// --- brute-force oracle ----------------------------------------------------

fn clause_sat(mask: u64, clause: &[SatLit]) -> bool {
    clause
        .iter()
        .any(|&l| ((mask >> l.var()) & 1 == 1) != l.is_negative())
}

fn brute_sat(n: usize, clauses: &[Vec<SatLit>]) -> bool {
    (0u64..(1u64 << n)).any(|mask| clauses.iter().all(|c| clause_sat(mask, c)))
}

fn brute_full_model_count(n: usize, clauses: &[Vec<SatLit>]) -> usize {
    (0u64..(1u64 << n))
        .filter(|&mask| clauses.iter().all(|c| clause_sat(mask, c)))
        .count()
}

// --- generators ------------------------------------------------------------

/// A CNF as raw `(var, positive)` literals grouped into clauses.
type RawCnf = Vec<Vec<(u32, bool)>>;
/// A generated engine case: atom count, per-atom fact choice, and raw clauses.
type EngineCase = (usize, Vec<u8>, RawCnf);

/// A random CNF: `n` in 1..=6 variables, up to 14 clauses of 1..=4 literals.
fn instance() -> impl Strategy<Value = (usize, RawCnf)> {
    (1usize..=6).prop_flat_map(|n| {
        let lit = (0u32..(n as u32), any::<bool>());
        let clause = prop::collection::vec(lit, 1..=4);
        (Just(n), prop::collection::vec(clause, 0..=14))
    })
}

fn to_clauses(raw: &[Vec<(u32, bool)>]) -> Vec<Vec<SatLit>> {
    raw.iter()
        .map(|c| c.iter().map(|&(v, p)| SatLit::new(v, p)).collect())
        .collect()
}

fn to_cnf(n: usize, raw: &[Vec<(u32, bool)>]) -> Cnf {
    let mut cnf = Cnf::new(n);
    for c in to_clauses(raw) {
        cnf.add_clause(c);
    }
    cnf
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(800))]

    /// Soundness AND completeness: our verdict matches exhaustive search.
    #[test]
    fn sat_matches_bruteforce((n, raw) in instance()) {
        let cnf = to_cnf(n, &raw);
        let clauses = to_clauses(&raw);
        prop_assert_eq!(sat::solve(&cnf).is_some(), brute_sat(n, &clauses));
    }

    /// Every returned model actually satisfies the formula.
    #[test]
    fn returned_model_is_valid((n, raw) in instance()) {
        let cnf = to_cnf(n, &raw);
        if let Some(model) = sat::solve(&cnf) {
            for clause in &to_clauses(&raw) {
                prop_assert!(clause.iter().any(|&l| model[l.var() as usize] != l.is_negative()));
            }
        }
    }

    /// all-SAT model counting equals the exact number of satisfying assignments.
    #[test]
    fn model_count_is_exact((n, raw) in instance()) {
        let cnf = to_cnf(n, &raw);
        let clauses = to_clauses(&raw);
        let all_vars: Vec<Var> = (0..n as Var).collect();
        let counted = sat::models_upto(&cnf, &all_vars, 1usize << n);
        prop_assert_eq!(counted, brute_full_model_count(n, &clauses));
    }
}

// --- engine-level invariant ------------------------------------------------

fn origin() -> Origin {
    Origin {
        source: "<prop>".into(),
        line: 0,
        axiom: None,
        kind: "EXCLUSIVE",
    }
}

/// Build a random `Compiled` over `n` atoms: a partial (non-contradictory) fact
/// assignment plus random `Impossible` clauses. `fact_choice[i]` is 0=unknown,
/// 1=true, 2=false; `clauses[j]` is a list of (atom, negated) literals.
fn engine_instance() -> impl Strategy<Value = EngineCase> {
    (2usize..=6).prop_flat_map(|n| {
        let facts = prop::collection::vec(0u8..3, n);
        let lit = (0u32..(n as u32), any::<bool>());
        let clause = prop::collection::vec(lit, 1..=4);
        (Just(n), facts, prop::collection::vec(clause, 0..=10))
    })
}

fn build_compiled(n: usize, fact_choice: &[u8], raw: &[Vec<(u32, bool)>]) -> Compiled {
    let atoms: Vec<AtomKey> = (0..n)
        .map(|i| AtomKey {
            subject: "s".into(),
            predicate: alloc_p(i),
            object: None,
        })
        .collect();
    let facts: Vec<Fact> = fact_choice
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| match c {
            1 => Some(Fact { atom: i as AtomId, value: Value::True, origin: origin() }),
            2 => Some(Fact { atom: i as AtomId, value: Value::False, origin: origin() }),
            _ => None,
        })
        .collect();
    let clauses: Vec<Clause> = raw
        .iter()
        .map(|c| Clause {
            lits: c.iter().map(|&(v, neg)| Lit { atom: v, negated: neg }).collect(),
            origin: origin(),
        })
        .collect();
    Compiled {
        atoms,
        facts,
        clauses,
        rules: Vec::new(),
        checks: Vec::new(),
        pending_imports: Vec::new(),
    }
}

fn alloc_p(i: usize) -> String {
    format!("p{i}")
}

/// Independently re-encode the CNF (clauses as Impossible + facts as units) and
/// cross-check: a forward-pass CONFLICT implies the encoded system is UNSAT.
fn encode(compiled: &Compiled) -> Cnf {
    let mut cnf = Cnf::new(compiled.atoms.len());
    for clause in &compiled.clauses {
        cnf.add_clause(clause.lits.iter().map(|l| SatLit::new(l.atom, l.negated)).collect());
    }
    for f in &compiled.facts {
        cnf.add_clause(vec![match f.value {
            Value::True => SatLit::positive(f.atom),
            Value::False => SatLit::negative(f.atom),
        }]);
    }
    cnf
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// The forward pass never panics and yields a well-formed report; and a
    /// CONFLICT verdict is corroborated by the SAT encoding being UNSAT.
    #[test]
    fn forward_conflict_implies_unsat((n, facts, raw) in engine_instance()) {
        let compiled = build_compiled(n, &facts, &raw);
        let report = solve(&compiled);
        if report.status == Status::Conflict {
            prop_assert!(sat::solve(&encode(&compiled)).is_none());
        }
    }

    /// Solving is deterministic: the same program yields the same report.
    #[test]
    fn solve_is_deterministic((n, facts, raw) in engine_instance()) {
        let compiled = build_compiled(n, &facts, &raw);
        prop_assert_eq!(solve(&compiled), solve(&compiled));
    }

    /// Report::to_json always emits valid JSON, whatever the program.
    #[test]
    fn to_json_is_always_valid((n, facts, raw) in engine_instance()) {
        let json = solve(&build_compiled(n, &facts, &raw)).to_json();
        prop_assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok(), "{}", json);
    }
}
