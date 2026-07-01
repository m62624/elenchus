//! Property-based tests (proptest).
//!
//! The SAT core is cross-checked against an exhaustive brute-force oracle over
//! small random formulas: any disagreement on SAT/UNSAT, an invalid model, or a
//! wrong model count is a real bug. This is the cheapest strong evidence that the
//! CDCL implementation (watched literals, 1-UIP learning, backjumping) is correct
//! — much cheaper than DRAT proof checking and sufficient at our scale.

use elenchus_compiler::{AtomId, AtomKey, Check, Clause, Compiled, Fact, Lit, Origin, Rule, Value};
use elenchus_solver::sat::{self, Cnf, SatLit, Solved, Var};
use elenchus_solver::{
    CompileError, MemoryResolver, PortBinding, Status, TraceReason, compile_source,
    compile_source_with, solve, verify_source, verify_source_with, verify_with,
};
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

/// A raw assumption `(var, positive)` holds in `mask`.
fn assumption_ok(mask: u64, (v, p): (u32, bool)) -> bool {
    ((mask >> v) & 1 == 1) == p
}

/// Brute SAT of `clauses` restricted to assignments honoring all `assumptions`.
fn brute_sat_assuming(n: usize, clauses: &[Vec<SatLit>], assumptions: &[(u32, bool)]) -> bool {
    (0u64..(1u64 << n)).any(|mask| {
        assumptions.iter().all(|&a| assumption_ok(mask, a))
            && clauses.iter().all(|c| clause_sat(mask, c))
    })
}

// --- generators ------------------------------------------------------------

/// A CNF as raw `(var, positive)` literals grouped into clauses.
type RawCnf = Vec<Vec<(u32, bool)>>;
/// A generated engine case: atom count, per-atom fact choice, and raw clauses.
type EngineCase = (usize, Vec<u8>, RawCnf);

/// A random CNF: `n` in 1..=8 variables, up to 18 clauses of 1..=4 literals.
/// (n≤8 keeps the 2^n brute-force oracle cheap while widening coverage.)
fn instance() -> impl Strategy<Value = (usize, RawCnf)> {
    (1usize..=8).prop_flat_map(|n| {
        let lit = (0u32..(n as u32), any::<bool>());
        let clause = prop::collection::vec(lit, 1..=4);
        (Just(n), prop::collection::vec(clause, 0..=18))
    })
}

/// An [`instance`] paired with a random set of 0..=n assumption literals over its
/// variables (possibly redundant or self-contradictory — all valid to assume).
fn instance_with_assumptions() -> impl Strategy<Value = (usize, RawCnf, Vec<(u32, bool)>)> {
    instance().prop_flat_map(|(n, raw)| {
        let lit = (0u32..(n as u32), any::<bool>());
        (Just(n), Just(raw), prop::collection::vec(lit, 0..=n))
    })
}

fn to_assumptions(asm: &[(u32, bool)]) -> Vec<SatLit> {
    asm.iter().map(|&(v, p)| SatLit::new(v, p)).collect()
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(700))]

    /// Solving under assumptions agrees with brute force on SAT/UNSAT.
    #[test]
    fn assuming_matches_bruteforce((n, raw, asm) in instance_with_assumptions()) {
        let cnf = to_cnf(n, &raw);
        let clauses = to_clauses(&raw);
        let got_sat = matches!(sat::solve_assuming(&cnf, &to_assumptions(&asm)), Solved::Sat(_));
        prop_assert_eq!(got_sat, brute_sat_assuming(n, &clauses, &asm));
    }

    /// A model returned under assumptions satisfies every clause AND every assumption.
    #[test]
    fn assuming_model_honors_clauses_and_assumptions((n, raw, asm) in instance_with_assumptions()) {
        let cnf = to_cnf(n, &raw);
        if let Solved::Sat(model) = sat::solve_assuming(&cnf, &to_assumptions(&asm)) {
            for clause in &to_clauses(&raw) {
                prop_assert!(clause.iter().any(|&l| model[l.var() as usize] != l.is_negative()));
            }
            for &(v, p) in &asm {
                prop_assert_eq!(model[v as usize], p);
            }
        }
    }

    /// An unsat core is a subset of the assumptions and is itself sufficient:
    /// `cnf ∧ core` is unsatisfiable (the cheap, faithful core contract).
    #[test]
    fn assuming_core_is_a_sufficient_subset((n, raw, asm) in instance_with_assumptions()) {
        let cnf = to_cnf(n, &raw);
        let clauses = to_clauses(&raw);
        let assumptions = to_assumptions(&asm);
        if let Solved::Unsat(core) = sat::solve_assuming(&cnf, &assumptions) {
            for l in &core {
                prop_assert!(assumptions.contains(l), "core lit {:?} not an assumption", l);
            }
            let core_pairs: Vec<(u32, bool)> =
                core.iter().map(|l| (l.var(), !l.is_negative())).collect();
            prop_assert!(!brute_sat_assuming(n, &clauses, &core_pairs), "core not sufficient");
        }
    }
}

// --- engine-level invariant ------------------------------------------------

fn origin() -> Origin {
    Origin {
        source: "<prop>".into(),
        line: 0,
        premise: None,
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
            domain: "t".into(),
            subject: "s".into(),
            predicate: Some(alloc_p(i)),
            object: None,
        })
        .collect();
    let facts: Vec<Fact> = fact_choice
        .iter()
        .enumerate()
        .filter_map(|(i, &c)| match c {
            1 => Some(Fact {
                atom: i as AtomId,
                value: Value::True,
                origin: origin(),
                soft: false,
            }),
            2 => Some(Fact {
                atom: i as AtomId,
                value: Value::False,
                origin: origin(),
                soft: false,
            }),
            _ => None,
        })
        .collect();
    let clauses: Vec<Clause> = raw
        .iter()
        .map(|c| Clause {
            lits: c
                .iter()
                .map(|&(v, neg)| Lit {
                    atom: v,
                    negated: neg,
                })
                .collect(),
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
        unused_imports: Vec::new(),
        consumed: Vec::new(),
        placeholders: Vec::new(),
        unwitnessed_exists: Vec::new(),
        justifications: Vec::new(),
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
        cnf.add_clause(
            clause
                .lits
                .iter()
                .map(|l| SatLit::new(l.atom, l.negated))
                .collect(),
        );
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

/// Same random program, but with a bidirectional `CHECK` so the backward pass and
/// the assumption-based unsat-core extraction actually run.
fn build_checked(n: usize, fact_choice: &[u8], raw: &[Vec<(u32, bool)>]) -> Compiled {
    let mut c = build_compiled(n, fact_choice, raw);
    c.checks = vec![Check {
        subject: None,
        bidirectional: true,
    }];
    c
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// The backward pass + assumption-based core extraction never panic, and a
    /// reported unsat core only appears alongside a genuine CONFLICT whose encoded
    /// system is UNSAT — guarding the selector-assumption rewire of the core.
    #[test]
    fn reported_unsat_core_implies_unsat((n, facts, raw) in engine_instance()) {
        let compiled = build_checked(n, &facts, &raw);
        let report = solve(&compiled);
        if !report.unsat_core.is_empty() {
            prop_assert_eq!(report.status, Status::Conflict);
            prop_assert!(sat::solve(&encode(&compiled)).is_none());
        }
    }
}

// --- derivation trace (conflict explainability) ----------------------------

/// A list of rules, each `antecedent literals → one consequent literal`.
type RawRules = Vec<(Vec<(u32, bool)>, (u32, bool))>;

/// A program that also has random forward-chaining rules, so a conflict's trace
/// can include `Derived` steps (real chains), not just asserted facts.
fn trace_instance() -> impl Strategy<Value = (usize, Vec<u8>, RawCnf, RawRules)> {
    (2usize..=6).prop_flat_map(|n| {
        let lit = (0u32..(n as u32), any::<bool>());
        let facts = prop::collection::vec(0u8..3, n);
        let clause = prop::collection::vec(lit.clone(), 1..=3);
        let rule = (prop::collection::vec(lit.clone(), 1..=2), lit);
        (
            Just(n),
            facts,
            prop::collection::vec(clause, 0..=6),
            prop::collection::vec(rule, 0..=6),
        )
    })
}

fn build_with_rules(
    n: usize,
    fact_choice: &[u8],
    raw: &[Vec<(u32, bool)>],
    rules: &RawRules,
) -> Compiled {
    let mut c = build_compiled(n, fact_choice, raw);
    c.rules = rules
        .iter()
        .map(|rule| {
            let (cv, cneg) = rule.1;
            Rule {
                antecedent: rule
                    .0
                    .iter()
                    .map(|&(v, neg)| Lit {
                        atom: v,
                        negated: neg,
                    })
                    .collect(),
                consequent: vec![Lit {
                    atom: cv,
                    negated: cneg,
                }],
                exceptions: Vec::new(),
                origin: origin(),
            }
        })
        .collect();
    c
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    /// A conflict's derivation trace is well-formed: every atom appears once, and
    /// each `Derived` step's supports appear *earlier* (facts before the rules
    /// built on them). That is exactly a valid topological order — no duplicates,
    /// no forward references, no cycles — which is what makes the `why:` chain
    /// readable top-to-bottom.
    #[test]
    fn conflict_trace_is_topologically_well_formed(
        (n, facts, raw, rules) in trace_instance()
    ) {
        let report = solve(&build_with_rules(n, &facts, &raw, &rules));
        for conflict in &report.conflicts {
            let mut seen: Vec<String> = Vec::new();
            for step in &conflict.trace {
                prop_assert!(!seen.contains(&step.atom), "duplicate trace atom {}", step.atom);
                if let TraceReason::Derived { from, .. } = &step.reason {
                    for f in from {
                        prop_assert!(seen.contains(f), "support `{}` used before it appears", f);
                    }
                }
                seen.push(step.atom.clone());
            }
        }
    }
}

// --- near-duplicate atom hints: detector must match its spec exactly --------
// An independent reference implementation of the documented heuristic. The
// proptest asserts the engine's emitted hints equal this reference over random
// programs — so the detector has no false positives AND no false negatives
// relative to its spec (the real false-positive argument lives in the spec
// itself: signal A is fold-equality, signal B is a tiny same-subject edit in a
// cased script — see the unit tests for concrete English/Russian/CJK cases).

/// Fold like the solver: join with spaces, lowercase, `_`/whitespace → one space.
fn ref_fold(s: &str, p: &str, o: Option<&str>) -> Vec<char> {
    let mut raw = String::new();
    raw.push_str(s);
    raw.push(' ');
    raw.push_str(p);
    if let Some(o) = o {
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
            out.extend(ch.to_lowercase());
            prev_space = false;
        }
    }
    if out.last() == Some(&' ') {
        out.pop();
    }
    out
}

fn ref_lev(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        core::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Reference predicate, mirroring the solver's `atoms_look_similar`.
fn ref_close(a: &(String, String, Option<String>), b: &(String, String, Option<String>)) -> bool {
    let fa = ref_fold(&a.0, &a.1, a.2.as_deref());
    let fb = ref_fold(&b.0, &b.1, b.2.as_deref());
    if fa == fb {
        return true; // signal A
    }
    let cased = |f: &[char]| f.iter().all(|&c| c == ' ' || c.is_lowercase());
    if !cased(&fa) || !cased(&fb) || a.0 != b.0 || fa.len().abs_diff(fb.len()) > 1 {
        return false;
    }
    let min_len = fa.len().min(fb.len());
    min_len >= 5 && ref_lev(&fa, &fb) == 1
}

fn ref_label(a: &(String, String, Option<String>)) -> String {
    match &a.2 {
        Some(o) => format!("p.{} {} {}", a.0, a.1, o),
        None => format!("p.{} {}", a.0, a.1),
    }
}

/// A random atom drawn from small token pools that deliberately include
/// near-duplicates (so hints both fire and don't), plus split forms like
/// (`rolled`, `back`) vs the single token `rolled_back` to exercise signal A.
fn ref_atom() -> impl Strategy<Value = (String, String, Option<String>)> {
    let subj = prop::sample::select(vec!["x", "auth"]);
    let pred = prop::sample::select(vec![
        "tested",
        "tsted",
        "staging",
        "rolled_back",
        "rolled",
        "fuel",
        "fuels",
        "lead",
        "dev",
        "qa",
    ]);
    let obj = prop::sample::select(vec!["", "back", "ready", "qa"]);
    (subj, pred, obj).prop_map(|(s, p, o)| {
        let obj = if o.is_empty() {
            None
        } else {
            Some(o.to_string())
        };
        (s.to_string(), p.to_string(), obj)
    })
}

/// Normalize a hint pair to an unordered (min, max) tuple for set comparison
/// (the engine's a/b order follows atom-id order, not string order).
fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// The engine's near-duplicate hints equal the independent reference set —
    /// exactly, with no spurious extras (false positives) and none missing
    /// (false negatives). Run over hundreds of random multi-atom programs.
    #[test]
    fn near_duplicate_hints_match_reference(atoms in prop::collection::vec(ref_atom(), 2..=6)) {
        // Distinct atoms (the engine dedups identical triples).
        let mut distinct = atoms.clone();
        distinct.sort();
        distinct.dedup();

        // Reference: all distinct unordered pairs the spec calls "close".
        let mut expected: Vec<(String, String)> = Vec::new();
        for i in 0..distinct.len() {
            for j in (i + 1)..distinct.len() {
                if ref_close(&distinct[i], &distinct[j]) {
                    expected.push(pair(&ref_label(&distinct[i]), &ref_label(&distinct[j])));
                }
            }
        }
        expected.sort();
        expected.dedup();

        // Build a program and run the real engine.
        let mut src = String::from("DOMAIN p\n");
        for (s, p, o) in &atoms {
            match o {
                Some(o) => src.push_str(&format!("FACT {s} {p} {o}\n")),
                None => src.push_str(&format!("FACT {s} {p}\n")),
            }
        }
        src.push_str("CHECK\n");
        let report = verify_source("<prop>", &src).unwrap();

        let mut got: Vec<(String, String)> =
            report.hints.iter().map(|h| pair(&h.a, &h.b)).collect();
        // No self-pairs and no duplicate unordered pairs.
        for h in &report.hints {
            prop_assert_ne!(&h.a, &h.b);
        }
        let before = got.len();
        got.sort();
        got.dedup();
        prop_assert_eq!(before, got.len(), "duplicate hint pair emitted");

        prop_assert_eq!(got, expected, "engine hints differ from the reference");
    }
}

// --- full-stack panic-safety: parse → compile → solve never panics -----------
// As the language grows, the cheapest safety net is "arbitrary text in, an Ok or
// an Err out — never a panic". This fuzzes the whole pipeline through the public
// `verify_source`, mixing plausible statements with garbage so deep parser and
// compiler states are reached. A panic here fails the test automatically.

fn fuzz_ident() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "x", "y", "auth", "rel", "a", "b", "c", "tested", "is", "staging", "over_100",
    ])
    .prop_map(String::from)
}

fn fuzz_atom() -> impl Strategy<Value = String> {
    (fuzz_ident(), fuzz_ident(), prop::option::of(fuzz_ident())).prop_map(|(s, p, o)| match o {
        Some(o) => format!("{s} {p} {o}"),
        None => format!("{s} {p}"),
    })
}

fn fuzz_line() -> impl Strategy<Value = String> {
    prop_oneof![
        fuzz_atom().prop_map(|a| format!("FACT {a}")),
        fuzz_atom().prop_map(|a| format!("NOT {a}")),
        Just("CHECK".to_string()),
        fuzz_ident().prop_map(|s| format!("CHECK {s}")),
        fuzz_ident().prop_map(|s| format!("CHECK {s} BIDIRECTIONAL")),
        fuzz_ident().prop_map(|n| format!("IMPORT \"{n}.vrf\"")),
        (fuzz_ident(), fuzz_atom(), fuzz_atom())
            .prop_map(|(n, a, b)| format!("PREMISE {n}:\n    ONEOF\n        {a}\n        {b}")),
        (fuzz_ident(), fuzz_atom(), fuzz_atom())
            .prop_map(|(n, a, b)| format!("PREMISE {n}:\n    WHEN {a}\n    OR {b}\n    THEN {a}")),
        (fuzz_ident(), fuzz_atom(), fuzz_atom())
            .prop_map(|(n, a, b)| format!("RULE {n}:\n    WHEN {a}\n    THEN {b}")),
        "//[a-z ]{0,10}".prop_map(String::from),
        // raw garbage to hit error paths
        "[A-Za-z0-9 _.!?\"]{0,16}".prop_map(String::from),
    ]
}

fn fuzz_program() -> impl Strategy<Value = String> {
    prop::collection::vec(fuzz_line(), 0..=12).prop_map(|lines| {
        let mut s = lines.join("\n");
        s.push('\n');
        s
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(800))]

    /// Arbitrary program text never panics: the pipeline returns a parse/compile
    /// error or a well-formed report (exit code always 0/1/2).
    #[test]
    fn pipeline_never_panics_on_arbitrary_text(src in fuzz_program()) {
        if let Ok(report) = verify_source("<fuzz>", &src) {
            prop_assert!((0..=2).contains(&report.exit_code()));
        }
    }
}

// --- OR/AND implication lowering vs an exhaustive truth table -----------------
// The four AND/OR combinations of WHEN…THEN compile to `Impossible` clauses. This
// proves the lowering is *logically equivalent* to the implication: over every
// assignment to the atoms, "all compiled clauses hold" must equal "the implication
// holds". A small k keeps the 2^k enumeration cheap.

#[derive(Debug, Clone)]
struct ImplCase {
    k: usize,
    ante: Vec<(usize, bool)>, // (atom index, negated)
    ante_or: bool,
    cons: Vec<(usize, bool)>,
    cons_or: bool,
}

fn impl_case() -> impl Strategy<Value = ImplCase> {
    (2usize..=5).prop_flat_map(|k| {
        (
            Just(k),
            prop::collection::vec((0..k, any::<bool>()), 1..=3),
            any::<bool>(),
            prop::collection::vec((0..k, any::<bool>()), 1..=3),
            any::<bool>(),
        )
            .prop_map(|(k, ante, ante_or, cons, cons_or)| ImplCase {
                k,
                ante,
                ante_or,
                cons,
                cons_or,
            })
    })
}

fn build_impl_program(c: &ImplCase) -> String {
    let lit = |(i, neg): (usize, bool)| {
        let a = format!("x a{i}");
        if neg { format!("NOT {a}") } else { a }
    };
    let mut s = String::from("PREMISE p:\n");
    s += &format!("    WHEN {}\n", lit(c.ante[0]));
    for &l in &c.ante[1..] {
        s += &format!("    {} {}\n", if c.ante_or { "OR" } else { "AND" }, lit(l));
    }
    s += &format!("    THEN {}\n", lit(c.cons[0]));
    for &l in &c.cons[1..] {
        s += &format!("    {} {}\n", if c.cons_or { "OR" } else { "AND" }, lit(l));
    }
    s += "CHECK\n";
    s
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn or_and_implication_lowering_matches_truth_table(case in impl_case()) {
        let compiled = compile_source("<or>", &format!("DOMAIN o\n{}", build_impl_program(&case)))
            .expect("a single premise compiles");
        // atom index -> interned id
        let mut id_of = vec![None; case.k];
        for (id, key) in compiled.atoms.iter().enumerate() {
            if key.subject == "x"
                && let Some(i) = key
                    .predicate
                    .as_deref()
                    .and_then(|p| p.strip_prefix('a'))
                    .and_then(|n| n.parse::<usize>().ok())
                && i < case.k
            {
                id_of[i] = Some(id as u32);
            }
        }
        for mask in 0u32..(1u32 << case.k) {
            let bit = |i: usize| (mask >> i) & 1 == 1;
            let holds = |(i, neg): (usize, bool)| if neg { !bit(i) } else { bit(i) };
            let ante_holds = if case.ante_or {
                case.ante.iter().any(|&l| holds(l))
            } else {
                case.ante.iter().all(|&l| holds(l))
            };
            let cons_holds = if case.cons_or {
                case.cons.iter().any(|&l| holds(l))
            } else {
                case.cons.iter().all(|&l| holds(l))
            };
            let impl_ok = !ante_holds || cons_holds;
            // Every Impossible clause is satisfied iff its listed literals are not
            // all simultaneously true.
            let clauses_ok = compiled.clauses.iter().all(|cl| {
                !cl.lits.iter().all(|l| {
                    let idx = id_of.iter().position(|&x| x == Some(l.atom)).unwrap();
                    if l.negated { !bit(idx) } else { bit(idx) }
                })
            });
            prop_assert_eq!(impl_ok, clauses_ok, "mask={} case={:?}", mask, case);
        }
    }
}

// --- external boolean ports (VAR / DEFAULT / values) -------------------------
// The eight port invariants (plan §7). The load-bearing one is the substitution
// oracle: a resolved port must be observationally equal to a plain `FACT`/`NOT`
// of the same proposition — ports add scheduling of truth, never new semantics.

/// A declared port: `(name, optional DEFAULT, negated-in-antecedent)`. Names are
/// `p0..pN`, distinct by construction.
type PortSpec = (String, Option<bool>, bool);

/// A bare port name that can never collide with the CAPS reserved words.
fn port_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,4}"
}

/// 1..=4 distinct ports, each with an optional `DEFAULT` and a use-polarity, plus
/// an external value per port (`None` = not supplied).
fn port_case() -> impl Strategy<Value = (Vec<PortSpec>, Vec<Option<bool>>)> {
    (1usize..=4)
        .prop_flat_map(|n| {
            let specs = prop::collection::vec((prop::option::of(any::<bool>()), any::<bool>()), n);
            (Just(n), specs)
        })
        .prop_flat_map(|(n, specs)| {
            let ports: Vec<PortSpec> = specs
                .into_iter()
                .enumerate()
                .map(|(i, (default, neg))| (format!("p{i}"), default, neg))
                .collect();
            let binds = prop::collection::vec(prop::option::of(any::<bool>()), n);
            (Just(ports), binds)
        })
}

fn bool_word(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

fn lit_src(name: &str, neg: bool) -> String {
    if neg {
        format!("NOT {name}")
    } else {
        name.to_string()
    }
}

/// The shared body: a single premise whose antecedent is the conjunction of every
/// port's (possibly negated) literal, forcing `goal x` — which `NOT goal x` denies.
/// So the verdict is a pure function of the resolved port values (CONFLICT when the
/// antecedent holds, WARNING when an UNKNOWN port leaves it undecided, else CONSISTENT).
fn port_body(ports: &[PortSpec]) -> String {
    let mut s = String::from("NOT goal x\nPREMISE g:\n");
    let (first, rest) = ports.split_first().unwrap();
    s.push_str(&format!("    WHEN {}\n", lit_src(&first.0, first.2)));
    for (name, _, neg) in rest {
        s.push_str(&format!("    AND {}\n", lit_src(name, *neg)));
    }
    s.push_str("    THEN goal x\nCHECK\n");
    s
}

/// Program A: ports declared with their `DEFAULT`s, values supplied externally.
fn build_ports(ports: &[PortSpec]) -> String {
    let mut s = String::from("DOMAIN d\n");
    for (name, default, _) in ports {
        s.push_str(&format!("VAR {name}"));
        if let Some(d) = default {
            s.push_str(&format!(" DEFAULT {}", bool_word(*d)));
        }
        s.push('\n');
    }
    s.push_str(&port_body(ports));
    s
}

/// Program B (the oracle): the *same* logic, but every resolved port is written out
/// as a literal `FACT name` / `NOT name` (and an UNKNOWN port is simply omitted),
/// with no external values. Ports keep their `VAR` so the bare atoms stay declared,
/// but carry no `DEFAULT` — resolution is done by hand here.
fn build_substituted(ports: &[PortSpec], binds: &[Option<bool>]) -> String {
    let mut s = String::from("DOMAIN d\n");
    for (name, _, _) in ports {
        s.push_str(&format!("VAR {name}\n"));
    }
    for ((name, default, _), b) in ports.iter().zip(binds) {
        if let Some(v) = b.or(*default) {
            s.push_str(&format!("{} {name}\n", if v { "FACT" } else { "NOT" }));
        }
    }
    s.push_str(&port_body(ports));
    s
}

/// External bindings from the supplied values (origin is irrelevant to the verdict).
fn inputs_from(ports: &[PortSpec], binds: &[Option<bool>]) -> Vec<(String, PortBinding)> {
    ports
        .iter()
        .zip(binds)
        .filter_map(|((name, _, _), b)| {
            b.map(|value| {
                (
                    name.clone(),
                    PortBinding {
                        value,
                        origin: "CLI".into(),
                    },
                )
            })
        })
        .collect()
}

/// The verdict-relevant shape of a report: status plus each conflict's sorted atom
/// labels. Atom *labels* are identical between a port and its `FACT` substitution
/// (same atom key), whereas the fact's *origin* (VAR vs FACT) is not — so we compare
/// the shape, not the trace provenance.
fn verdict_shape(r: &elenchus_solver::Report) -> (Status, Vec<Vec<String>>) {
    let mut conflicts: Vec<Vec<String>> = r
        .conflicts
        .iter()
        .map(|c| {
            let mut a = c.atoms.clone();
            a.sort();
            a
        })
        .collect();
    conflicts.sort();
    (r.status, conflicts)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    /// (1,3,4) Substitution oracle: a program with `VAR` ports + external/`DEFAULT`
    /// values yields the same verdict as the program where each resolved port is a
    /// literal `FACT`/`NOT` (external overrides `DEFAULT`; an unset port with no
    /// `DEFAULT` stays UNKNOWN). Ports add no semantics beyond `FACT`/`NOT`.
    #[test]
    fn ports_are_equivalent_to_fact_substitution((ports, binds) in port_case()) {
        let a = verify_source_with("<a>", &build_ports(&ports), &inputs_from(&ports, &binds))
            .map_err(|e| TestCaseError::fail(format!("A: {e}")))?;
        let b = verify_source_with("<b>", &build_substituted(&ports, &binds), &[])
            .map_err(|e| TestCaseError::fail(format!("B: {e}")))?;
        prop_assert_eq!(verdict_shape(&a), verdict_shape(&b));
    }

    /// (7) Determinism: identical program + inputs ⇒ byte-identical result.
    #[test]
    fn port_resolution_is_deterministic((ports, binds) in port_case()) {
        let src = build_ports(&ports);
        let inputs = inputs_from(&ports, &binds);
        prop_assert_eq!(
            verify_source_with("<d>", &src, &inputs),
            verify_source_with("<d>", &src, &inputs)
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// (2) Two disagreeing bindings on one port are *always* a hard error — never a
    /// silent pick. Determinism over override.
    #[test]
    fn disagreeing_bindings_always_conflict(name in port_name(), v in any::<bool>()) {
        let src = format!("DOMAIN d\nVAR {name}\nCHECK\n");
        let inputs = vec![
            (name.clone(), PortBinding { value: v, origin: "CLI".into() }),
            (name.clone(), PortBinding { value: !v, origin: "api".into() }),
        ];
        let err = verify_source_with("<c>", &src, &inputs).unwrap_err();
        prop_assert!(matches!(err, CompileError::PortConflict { .. }), "{err:?}");
    }

    /// (5) A used bare proposition with no `VAR` is a hard `UndeclaredPort` error.
    #[test]
    fn undeclared_bare_proposition_is_an_error(name in port_name()) {
        let src = format!("DOMAIN d\nFACT {name}\nCHECK\n");
        let err = verify_source_with("<u>", &src, &[]).unwrap_err();
        prop_assert!(matches!(err, CompileError::UndeclaredPort { .. }), "{err:?}");
    }

    /// (6) A bare name declared in two *domains* (each its own file, aggregated by
    /// import) is ambiguous once externally set — which port the value means is
    /// undecidable, so it is a hard error.
    #[test]
    fn cross_domain_name_is_ambiguous_when_set(name in port_name(), v in any::<bool>()) {
        let mut r = MemoryResolver::new();
        r.add("root.vrf", "DOMAIN r\nIMPORT \"a.vrf\"\nIMPORT \"b.vrf\"\nCHECK\n")
            .add("a.vrf", &format!("DOMAIN a\nVAR {name}\n"))
            .add("b.vrf", &format!("DOMAIN b\nVAR {name}\n"));
        let inputs = vec![(name.clone(), PortBinding { value: v, origin: "CLI".into() })];
        let err = verify_with("root.vrf", &r, &inputs).unwrap_err();
        prop_assert!(matches!(err, CompileError::AmbiguousPort { .. }), "{err:?}");
    }

    /// (8) Encoding size stays linear in the number of ports — no combinatorial
    /// blow-up. Each of the `n` one-port premises lowers to a single clause.
    #[test]
    fn clause_count_is_linear_in_ports(n in 1usize..=12) {
        let mut src = String::from("DOMAIN d\n");
        for i in 0..n {
            src.push_str(&format!("VAR p{i}\n"));
        }
        src.push_str("NOT goal x\n");
        for i in 0..n {
            src.push_str(&format!("PREMISE g{i}:\n    WHEN p{i}\n    THEN goal x\n"));
        }
        src.push_str("CHECK\n");
        let compiled = compile_source_with("<l>", &src, &[]).unwrap();
        prop_assert!(
            compiled.clauses.len() <= 2 * n + 2,
            "clauses={} n={}",
            compiled.clauses.len(),
            n
        );
    }
}

// --- qualified domain.port keys + multi-word atom injection ------------------
// Extends the eight port invariants for cross-domain addressing: a `domain.`
// prefix pins a port to one domain (and is identical to the bare key when the name
// is unique), and a multi-word external key injects an *atom* exactly like a
// hand-written FACT/NOT — the same substitution oracle, lifted to atoms.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// A `domain.`-qualified key is observationally identical to the bare key when
    /// the port name is unique (one domain): qualification is free to add.
    #[test]
    fn qualified_key_equals_bare_when_unique((ports, binds) in port_case()) {
        let src = build_ports(&ports);
        let bare = inputs_from(&ports, &binds);
        let qual: Vec<(String, PortBinding)> = bare
            .iter()
            .map(|(k, b)| (format!("d.{k}"), b.clone()))
            .collect();
        prop_assert_eq!(
            verify_source_with("<q>", &src, &bare),
            verify_source_with("<q>", &src, &qual)
        );
    }

    /// Two imported domains declare the same port name; qualified keys set each
    /// independently (no AmbiguousPort), matching a hand FACT/NOT substitution per
    /// domain. This is the exact wall the decoded test session hit, now resolvable.
    #[test]
    fn qualified_disambiguates_cross_domain_collision(va in any::<bool>(), vb in any::<bool>()) {
        let root = "DOMAIN r\nIMPORT \"a.vrf\"\nIMPORT \"b.vrf\"\nCHECK\n";
        let body = |dom: &str| {
            format!("DOMAIN {dom}\nVAR k\nNOT goal x\nPREMISE g:\n    WHEN k\n    THEN goal x\n")
        };
        let mut ports = MemoryResolver::new();
        ports
            .add("root.vrf", root)
            .add("a.vrf", &body("a"))
            .add("b.vrf", &body("b"));
        let inputs = vec![
            ("a.k".to_string(), PortBinding { value: va, origin: "CLI".into() }),
            ("b.k".to_string(), PortBinding { value: vb, origin: "CLI".into() }),
        ];
        let got = verify_with("root.vrf", &ports, &inputs)
            .map_err(|e| TestCaseError::fail(format!("ports: {e}")))?;

        // Oracle: the same two files, each port value written out as a FACT/NOT.
        let sub = |dom: &str, v: bool| {
            format!(
                "DOMAIN {dom}\nVAR k\n{} k\nNOT goal x\nPREMISE g:\n    WHEN k\n    THEN goal x\n",
                if v { "FACT" } else { "NOT" }
            )
        };
        let mut subr = MemoryResolver::new();
        subr.add("root.vrf", root)
            .add("a.vrf", &sub("a", va))
            .add("b.vrf", &sub("b", vb));
        let want = verify_with("root.vrf", &subr, &[])
            .map_err(|e| TestCaseError::fail(format!("sub: {e}")))?;
        prop_assert_eq!(verdict_shape(&got), verdict_shape(&want));
    }

    /// A multi-word external key asserting an atom equals an in-file `FACT`/`NOT`
    /// of that atom — external atom injection adds no semantics beyond the fact.
    #[test]
    fn multiword_external_key_equals_fact(v in any::<bool>()) {
        let logic =
            "NOT motor runs\nPREMISE g:\n    WHEN engine has_fuel\n    THEN motor runs\nCHECK\n";
        let ext = verify_source_with(
            "<e>",
            &format!("DOMAIN d\n{logic}"),
            &[("engine has_fuel".to_string(), PortBinding { value: v, origin: "CLI".into() })],
        )
        .map_err(|e| TestCaseError::fail(format!("ext: {e}")))?;
        let inline = verify_source(
            "<i>",
            &format!("DOMAIN d\n{} engine has_fuel\n{logic}", if v { "FACT" } else { "NOT" }),
        )
        .map_err(|e| TestCaseError::fail(format!("inline: {e}")))?;
        prop_assert_eq!(verdict_shape(&ext), verdict_shape(&inline));
    }
}

// --- EXISTS oracle ---------------------------------------------------------

/// 2..=4 elements, each with a "force NOT this instance" flag — so the
/// at-least-one is sometimes unsatisfiable (CONFLICT), sometimes not.
fn exists_case() -> impl Strategy<Value = (usize, Vec<bool>)> {
    (2usize..=4).prop_flat_map(|k| (Just(k), prop::collection::vec(any::<bool>(), k)))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// EXISTS over a declared SET yields the same verdict as a hand-written ATLEAST
    /// of the per-element instantiations — ∃ adds no semantics beyond at-least-one
    /// (same atoms, same single clause), it just generates the list from the set.
    #[test]
    fn exists_equals_hand_written_atleast((k, forced) in exists_case()) {
        let names: Vec<String> = (0..k).map(|i| format!("e{i}")).collect();
        let facts: String = names
            .iter()
            .zip(&forced)
            .filter(|(_, f)| **f)
            .map(|(n, _)| format!("NOT {n} p o\n"))
            .collect();
        let set: String = names.iter().map(|n| format!("    {n}\n")).collect();
        let atleast: String = names.iter().map(|n| format!("        {n} p o\n")).collect();
        let a = verify_source(
            "<ex>",
            &format!("DOMAIN t\n{facts}SET hs\n{set}PREMISE x:\n    EXISTS h IN hs\n        h p o\nCHECK\n"),
        )
        .map_err(|e| TestCaseError::fail(format!("ex: {e}")))?;
        let b = verify_source(
            "<al>",
            &format!("DOMAIN t\n{facts}PREMISE x:\n    ATLEAST\n{atleast}CHECK\n"),
        )
        .map_err(|e| TestCaseError::fail(format!("al: {e}")))?;
        prop_assert_eq!(verdict_shape(&a), verdict_shape(&b));
    }
}
