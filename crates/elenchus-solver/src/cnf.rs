//! Encode the IR (facts, premises, rules) as CNF for the backward (all-SAT) pass.
use crate::sat;
use alloc::vec;
use alloc::vec::Vec;
use elenchus_compiler::{AtomId, Compiled, Fact, Lit, Rule, Value};

/// One IR literal as it enters a CNF clause, with the polarity flip folded in: a
/// surface-positive literal `L` becomes `¬L`. This is exactly what both a PREMISE
/// `Impossible([L..])` (== ¬L1 ∨ … ∨ ¬Ln) and a RULE antecedent need. (`SatLit::new`'s
/// second argument is the *positive* flag, so passing `negated` performs the flip.)
pub(crate) fn clause_lit(l: &Lit) -> sat::SatLit {
    sat::SatLit::new(l.atom, l.negated)
}

/// The unit literal a confident fact contributes: `TRUE → positive`, `FALSE → negative`.
pub(crate) fn fact_lit(f: &Fact) -> sat::SatLit {
    match f.value {
        Value::True => sat::SatLit::positive(f.atom),
        Value::False => sat::SatLit::negative(f.atom),
    }
}

/// The CNF clause one rule consequent contributes: `WHEN A.. THEN C` == `(¬A1 ∨ … ∨ C)`.
/// The antecedent literals enter negated (via [`clause_lit`]); the consequent enters
/// with its surface polarity. The single encoding of "rule ⇒ clause", shared by the
/// backward-pass CNF and the unsat-core constructs.
pub(crate) fn rule_consequent_clause(r: &Rule, cons: &Lit) -> Vec<sat::SatLit> {
    let mut lits: Vec<sat::SatLit> = r.antecedent.iter().map(clause_lit).collect();
    lits.push(sat::SatLit::new(cons.atom, !cons.negated));
    lits
}

/// Encode the premises (`Impossible` clauses), rules (as implications), and
/// confident facts (as unit clauses) into CNF for the backward pass. Also
/// returns the constrained atoms (those appearing in a clause or rule) to
/// project model counting onto.
pub(crate) fn build_cnf(c: &Compiled) -> (sat::Cnf, Vec<sat::Var>) {
    let mut cnf = sat::Cnf::new(c.atoms.len());
    let mut constrained = vec![false; c.atoms.len()];

    // Premises and rules constrain every atom they mention; facts pin a value but do
    // not by themselves make an atom a candidate for the underdetermined witness, so
    // only these two add to `constrained`.
    let add_constraining =
        |cnf: &mut sat::Cnf, constrained: &mut [bool], lits: Vec<sat::SatLit>| {
            for l in &lits {
                constrained[l.var() as usize] = true;
            }
            cnf.add_clause(lits);
        };

    // Impossible([L1..Ln]) == (¬L1 ∨ … ∨ ¬Ln).
    for clause in &c.clauses {
        add_constraining(
            &mut cnf,
            &mut constrained,
            clause.lits.iter().map(clause_lit).collect(),
        );
    }
    // RULE WHEN A.. THEN C.. == for each C: (¬A1 ∨ … ∨ C).
    for r in &c.rules {
        for cons in &r.consequent {
            add_constraining(&mut cnf, &mut constrained, rule_consequent_clause(r, cons));
        }
    }
    // Confident facts as unit clauses (not marked constrained — see above).
    for f in &c.facts {
        cnf.add_clause(vec![fact_lit(f)]);
    }

    let project = (0..c.atoms.len() as AtomId)
        .filter(|&a| constrained[a as usize])
        .collect();
    (cnf, project)
}
