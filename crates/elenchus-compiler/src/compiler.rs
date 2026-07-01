//! The `Compiler`: accumulates statements from one or more sources, interns atoms,
//! desugars surface constructs onto the single `Impossible` primitive, resolves
//! ports, and emits the canonical IR. Preparation only — no solving.
use crate::hash_hex;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use elenchus_parser::{Atom, Body, Conn, ExistsDomain, ListOp, Located, Quant, Statement, kw};

use crate::closure::close;
use crate::domain::DomainCtx;
use crate::error::{CompileError, UnknownValue, did_you_mean, nearest_set_suggestion};
use crate::ir::{
    AtomId, AtomKey, Check, Clause, Compiled, Fact, Justification, Lit, Origin, PlaceholderInfo,
    PlaceholderStatus, PortBinding, Rule, UnwitnessedExists, Value,
};
use crate::ports::{PortDecl, PortRef, parse_port_ref};
use crate::resolver::{ResolvedFile, extract_domain, parse_tagged};
use crate::sig::{
    RawClause, RawFact, RawJustification, RawLit, RawRule, canonical_body, clause_sig, key_sig,
    list_kind, quant_sig, raw_lits,
};
use crate::subst::{subst_atom, subst_body};

// --- compiler --------------------------------------------------------------

/// Accumulates statements from one or more sources, then interns + emits the IR.
#[derive(Default)]
pub struct Compiler {
    keys: BTreeSet<AtomKey>,
    facts: Vec<RawFact>,
    clauses: Vec<RawClause>,
    rules: Vec<RawRule>,
    checks: Vec<Check>,
    pending_imports: Vec<String>,
    /// (source, name) → content hash of its body, for redefinition detection.
    /// Scoped per source: premise/rule names are labels, not global identifiers,
    /// so different files (domains) may reuse a name. A clash is only an error
    /// *within the same source*.
    defined: BTreeMap<(String, String), String>,
    /// dedup of identical clauses by canonical content hash.
    clause_sigs: BTreeSet<String>,
    /// dedup of identical facts by (key, value).
    fact_sigs: BTreeSet<String>,
    /// Closed-world value sets declared by `ONEOF`: `(domain, subject, predicate)`
    /// → the set of legal objects. Once a variable's values are enumerated by a
    /// `ONEOF`, a reference to that variable with an object *outside* the set is a
    /// compile error (a likely typo), not a silent new atom. Only `ONEOF` members
    /// that carry an object register here (binary atoms have no value slot to
    /// close). See [`Compiler::validate_closed_world`].
    oneof_values: BTreeMap<(String, String, String), BTreeSet<String>>,
    /// Declared `SET <name>` collections: name → elements, used to ground a
    /// `FOR EACH <binder> IN <name>` quantifier by instantiating the body once
    /// per element. Populated in a pre-pass so a `FOR EACH` may reference a set
    /// declared later in the file.
    sets: BTreeMap<String, Vec<String>>,
    /// Declared relation pairs: predicate → `(subject, object)` of every 3-part
    /// `FACT`, used to ground a `FOR EACH <a> <predicate> <b>` quantifier. Also a
    /// pre-pass, so the edges may be declared after the quantifier.
    relations: BTreeMap<String, Vec<(String, String)>>,
    /// Edge atoms consumed by a relation `FOR EACH` (e.g. each `a linked b`).
    /// They are *read as data* by the quantifier, so they are not idle facts —
    /// [`Compiler::finalize`] passes them to the report to suppress the ORPHAN
    /// lint.
    relation_consumed: BTreeSet<AtomKey>,
    /// Declared `VAR` ports, keyed by `(domain, name)`. Collected as statements are
    /// added (across every imported file), then resolved against external values in
    /// [`Compiler::resolve_ports`]. The first declaration of a `(domain, name)`
    /// wins; later duplicates are ignored.
    ports: BTreeMap<(String, String), PortDecl>,
    /// `PROVIDE` bindings seen in compiled sources (origin `"PROVIDE <file>"`).
    /// Each target's `domain.` prefix is already resolved to a canonical domain.
    /// They join the same conflict pool as external `--set`/API values in
    /// [`Compiler::resolve_ports`].
    provides: Vec<(PortRef, PortBinding)>,
    /// `EXISTS` premises that named no candidate (neither `SET` nor `WITNESS`).
    /// Inert for the SAT core (no clause), carried to the report as WARNINGs.
    unwitnessed_exists: Vec<UnwitnessedExists>,
    /// `FACT … BECAUSE <ground>` justifications. Inert for the SAT core (no clause);
    /// the solver checks the ground's value (FALSE → CONFLICT, UNKNOWN → WARNING).
    justifications: Vec<RawJustification>,
}

impl Compiler {
    /// A fresh, empty compiler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one source and accumulate its statements. `source` is a label used
    /// in provenance (e.g. a file name or `"<root>"`). The source must declare its
    /// `DOMAIN`; `IMPORT`s are recorded as pending (their domains cannot be bound
    /// without a [`Resolver`]), so a single source may only reference its own
    /// domain. Use [`compile`] for cross-domain references.
    pub fn add_source(&mut self, source: &str, src: &str) -> Result<(), CompileError> {
        let program = parse_tagged(source, src)?;
        let domain = extract_domain(&program, source)?;
        let mut aliases = BTreeMap::new();
        aliases.insert(domain.clone(), domain.clone());
        let ctx = DomainCtx {
            current: domain,
            aliases,
        };
        self.collect_decls(&program);
        self.apply_closures(&program, source)?;
        for stmt in &program.statements {
            match stmt {
                Statement::Domain(_) => {}
                Statement::Import { path, .. } => {
                    self.pending_imports.push(path.data.to_string());
                }
                other => self.add_statement(source, other, &ctx)?,
            }
        }
        Ok(())
    }

    /// Apply every `CLOSE <relation> <kind>`: replace the relation's pairs with
    /// their closure (via [`close`]) so a relation `FOR EACH` ranges over the
    /// closed relation. A pure compile-time graph pass — the solver never sees it.
    /// `TRANSITIVE` additionally rejects a cycle. Runs after [`collect_decls`] (the
    /// direct edges must be known) and before grounding.
    fn apply_closures(
        &mut self,
        program: &elenchus_parser::Program,
        source: &str,
    ) -> Result<(), CompileError> {
        for stmt in &program.statements {
            if let Statement::Close { relation, kind } = stmt {
                let pairs = self
                    .relations
                    .get(relation.data)
                    .cloned()
                    .unwrap_or_default();
                let closed = close(*kind, pairs).map_err(|node| CompileError::CyclicRelation {
                    file: source.to_string(),
                    line: relation.span.location_line(),
                    relation: relation.data.to_string(),
                    node,
                })?;
                self.relations.insert(relation.data.to_string(), closed);
            }
        }
        Ok(())
    }

    /// Pre-pass: record every `SET` and every relation pair (3-part `FACT`) so a
    /// `FOR EACH` may reference a set or relation declared anywhere in the same
    /// source, including after the quantifier.
    fn collect_decls(&mut self, program: &elenchus_parser::Program) {
        for stmt in &program.statements {
            match stmt {
                Statement::Set { name, elements } => {
                    self.sets.insert(
                        name.data.to_string(),
                        elements.iter().map(|e| e.data.to_string()).collect(),
                    );
                }
                Statement::Fact { atom: a, .. } => {
                    // Only a 3-part fact (`a rel b`) declares a relation pair; a bare
                    // proposition or a 2-word fact has no object (hence no predicate
                    // pair to record).
                    if let (Some(pred), Some(obj)) = (a.data.predicate, a.data.object) {
                        self.relations
                            .entry(pred.to_string())
                            .or_default()
                            .push((a.data.subject.to_string(), obj.to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    /// Compile one already-resolved file's statements under its domain context.
    pub(crate) fn add_resolved(&mut self, file: &ResolvedFile) -> Result<(), CompileError> {
        let program = parse_tagged(&file.path, &file.content)?;
        self.collect_decls(&program);
        self.apply_closures(&program, &file.path)?;
        for stmt in &program.statements {
            match stmt {
                Statement::Import { .. } | Statement::Domain(_) => {}
                other => self.add_statement(&file.path, other, &file.ctx)?,
            }
        }
        Ok(())
    }

    /// Route one statement (never `IMPORT`/`DOMAIN` — handled by the loaders) to
    /// the right accumulator, resolving atom domains through `ctx`.
    fn add_statement(
        &mut self,
        source: &str,
        stmt: &Statement,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        match stmt {
            // Handled by `add_source` / `load_recursive`, never reach here.
            Statement::Import { .. } | Statement::Domain(_) => {}
            Statement::Fact { atom: a, because } => {
                self.add_fact(source, a, Value::True, kw::FACT, false, ctx)?;
                if let Some(ground) = because {
                    self.add_justification(source, a, ground, ctx)?;
                }
            }
            Statement::Negation(a) => {
                self.add_fact(source, a, Value::False, kw::NOT, false, ctx)?
            }
            Statement::Assume(l) => {
                let value = if l.data.negated {
                    Value::False
                } else {
                    Value::True
                };
                // A soft assertion shares the FACT accumulator; the atom is the
                // literal's atom, the polarity its `NOT`, and `soft` marks it
                // retractable. The span is the whole `ASSUME` line.
                let located = elenchus_parser::Located {
                    data: l.data.atom.clone(),
                    span: l.span,
                };
                self.add_fact(source, &located, value, kw::ASSUME, true, ctx)?;
            }
            Statement::Check {
                subject,
                bidirectional,
            } => self.checks.push(Check {
                subject: subject.as_ref().map(|s| s.data.to_string()),
                bidirectional: *bidirectional,
            }),
            // Declared in the `collect_decls` / `apply_closures` pre-passes;
            // nothing to emit here.
            Statement::Set { .. } | Statement::Close { .. } => {}
            // A port declaration: record it under this file's domain (the first
            // declaration of a `(domain, name)` wins). It is resolved against
            // external values later, in `resolve_ports`.
            Statement::Var { name, default } => {
                self.ports
                    .entry((ctx.current.clone(), name.data.to_string()))
                    .or_insert(PortDecl {
                        default: *default,
                        source: source.to_string(),
                        line: name.span.location_line(),
                    });
            }
            // A value binding: resolve the target's `domain.` prefix against this
            // file's context (so an alias becomes a canonical domain; no prefix
            // stays `None` = search every domain, the historical behaviour), then
            // queue it into the conflict pool alongside any external values.
            Statement::Provide { atom, value } => {
                let a = &atom.data;
                let domain = match a.domain {
                    None => None,
                    Some(p) => Some(ctx.resolve(Some(p))?),
                };
                self.provides.push((
                    PortRef {
                        domain,
                        subject: a.subject.to_string(),
                        predicate: a.predicate.map(|p| p.to_string()),
                        object: a.object.map(|o| o.to_string()),
                    },
                    PortBinding {
                        value: *value,
                        origin: alloc::format!("PROVIDE {source}"),
                    },
                ));
            }
            Statement::Premise { name, quant, body } => {
                self.add_named(source, name, quant.as_ref(), body, false, ctx)?;
            }
            Statement::Rule { name, quant, body } => {
                self.add_named(source, name, quant.as_ref(), body, true, ctx)?;
            }
        }
        Ok(())
    }

    /// Record an atom identity in the shared universe (deduped by the `BTreeSet`).
    fn intern(&mut self, key: &AtomKey) {
        if !self.keys.contains(key) {
            self.keys.insert(key.clone());
        }
    }

    /// Accumulate a `FACT`/`NOT`; exact duplicates (same key+value+kind) are
    /// dropped as idempotent, while a `FACT` and a `NOT` on the same atom are
    /// both kept so the solver can report the CONFLICT.
    fn add_fact(
        &mut self,
        source: &str,
        a: &elenchus_parser::Located<Atom>,
        value: Value,
        kind: &'static str,
        soft: bool,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        let key = ctx.key(&a.data)?;
        self.intern(&key);
        let sig = alloc::format!(
            "{}|{}|{}|{}",
            key_sig(&key),
            matches!(value, Value::True) as u8,
            kind,
            "" // facts dedup ignores line; identical FACT twice is idempotent
        );
        if !self.fact_sigs.insert(sig) {
            return Ok(()); // exact duplicate fact — idempotent
        }
        self.facts.push(RawFact {
            key,
            value,
            origin: Origin {
                source: source.to_string(),
                line: a.span.location_line(),
                premise: None,
                kind,
            },
            soft,
        });
        Ok(())
    }

    /// Record a `FACT … BECAUSE <ground>` justification. The belief atom is already
    /// interned by the preceding [`Compiler::add_fact`]; the ground is interned here
    /// so it participates in the model (an otherwise-unconstrained ground stays
    /// UNKNOWN → the solver reports it rather than silently forcing it true). No
    /// clause is emitted — the check is evaluative, done by the solver.
    fn add_justification(
        &mut self,
        source: &str,
        belief: &Located<Atom>,
        ground: &Located<Atom>,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        let belief_key = ctx.key(&belief.data)?;
        let ground_key = ctx.key(&ground.data)?;
        self.intern(&ground_key);
        self.justifications.push(RawJustification {
            belief: belief_key,
            ground: ground_key,
            origin: Origin {
                source: source.to_string(),
                line: belief.span.location_line(),
                premise: None,
                kind: kw::BECAUSE,
            },
        });
        Ok(())
    }

    /// Handle a named construct (`PREMISE` or `RULE`). `is_rule` selects derivation
    /// vs checking. Returns an error on redefinition with a different body.
    fn add_named(
        &mut self,
        source: &str,
        name: &Located<&str>,
        quant: Option<&Quant>,
        body: &Body,
        is_rule: bool,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        let line = name.span.location_line();
        let name = name.data;
        // The redefinition hash covers the quantifier too, so two same-named
        // premises that differ only in their `FOR EACH` are still a redefinition.
        let mut canon = canonical_body(name, body, is_rule, ctx)?;
        if let Some(q) = quant {
            canon.push_str(&quant_sig(q));
        }
        let body_hash = hash_hex(canon.as_bytes());
        let key = (source.to_string(), name.to_string());
        match self.defined.get(&key) {
            Some(prev) if *prev == body_hash => return Ok(()), // identical → idempotent
            Some(_) => {
                // Same name + different body *in the same source* — a real mistake.
                return Err(CompileError::PremiseRedefinition {
                    name: name.to_string(),
                });
            }
            None => {
                self.defined.insert(key, body_hash);
            }
        }

        match quant {
            // Unquantified: emit the body once, as before.
            None => self.emit_named(source, name, line, body, is_rule, ctx),
            // `FOR EACH <binder> IN <set>`: instantiate the body once per element,
            // substituting the binder. Grounding is exactly `|set|` repetitions of
            // the *same* desugar — linear, never a domain product (a second binder
            // is unrepresentable in the grammar).
            Some(Quant::InSet { binder, set }) => {
                let elements = match self.sets.get(set.data) {
                    Some(els) => els.clone(),
                    None => {
                        return Err(CompileError::UnknownSet {
                            file: source.to_string(),
                            line: set.span.location_line(),
                            set: set.data.to_string(),
                            suggestion: nearest_set_suggestion(set.data, &self.sets),
                        });
                    }
                };
                for el in &elements {
                    let grounded = subst_body(body, &[(binder.data, el)]);
                    self.emit_named(source, name, line, &grounded, is_rule, ctx)?;
                }
                Ok(())
            }
            // `FOR EACH <a> <relation> <b>`: instantiate the body once per declared
            // FACT pair of that relation, binding `a`→subject, `b`→object. The pair
            // is pinned by data, so this is linear in the number of facts — never a
            // product of the domain with itself.
            Some(Quant::Relation {
                left,
                predicate,
                right,
            }) => {
                let pairs = self
                    .relations
                    .get(predicate.data)
                    .cloned()
                    .unwrap_or_default();
                for (subj, obj) in &pairs {
                    let grounded = subst_body(body, &[(left.data, subj), (right.data, obj)]);
                    self.emit_named(source, name, line, &grounded, is_rule, ctx)?;
                    // The edge atom is read as data by the quantifier, not idle —
                    // record it so the ORPHAN lint does not flag it.
                    if let Ok(k) = ctx.key(&Atom {
                        domain: None,
                        subject: subj,
                        predicate: Some(predicate.data),
                        object: Some(obj),
                    }) {
                        self.relation_consumed.insert(k);
                    }
                }
                Ok(())
            }
        }
    }

    /// Emit the clauses/rule for one (already-grounded) named construct's body.
    /// Split out of [`Compiler::add_named`] so a `FOR EACH` can call it once per
    /// element with the binder substituted, reusing the exact same desugar.
    fn emit_named(
        &mut self,
        source: &str,
        name: &str,
        line: u32,
        body: &Body,
        is_rule: bool,
        ctx: &DomainCtx,
    ) -> Result<(), CompileError> {
        if is_rule {
            // RULE always has an implication body (guaranteed by the grammar).
            if let Body::Impl {
                antecedent,
                ante_conn,
                consequent,
                cons_conn,
            } = body
            {
                // A rule *derives* its consequent; an `OR` consequent is not a
                // single fact to assert, so reject it (use a PREMISE instead).
                if *cons_conn == Conn::Or {
                    return Err(CompileError::RuleDisjunctiveConsequent {
                        name: name.to_string(),
                    });
                }
                let (ante, cons) = (raw_lits(antecedent, ctx)?, raw_lits(consequent, ctx)?);
                for l in ante.iter().chain(cons.iter()) {
                    self.intern(&l.key);
                }
                let origin = self.origin(source, line, Some(name), kw::RULE);
                match ante_conn {
                    // a ∧ b → C : one rule firing on the whole antecedent.
                    Conn::And => self.rules.push(RawRule {
                        antecedent: ante,
                        consequent: cons,
                        origin,
                    }),
                    // (a ∨ b) → C == (a → C) ∧ (b → C): one rule per antecedent.
                    Conn::Or => {
                        for a in &ante {
                            self.rules.push(RawRule {
                                antecedent: vec![a.clone()],
                                consequent: cons.clone(),
                                origin: origin.clone(),
                            });
                        }
                    }
                }
            }
            return Ok(());
        }

        match body {
            Body::List { op, atoms } => {
                let keys: Vec<AtomKey> = atoms
                    .iter()
                    .map(|a| ctx.key(&a.data))
                    .collect::<Result<_, _>>()?;
                for k in &keys {
                    self.intern(k);
                }
                let kind = list_kind(*op);
                let origin = self.origin(source, line, Some(name), kind);
                match op {
                    // EXCLUSIVE / FORBIDS: "at most one" → pairwise Impossible([a_i, a_j]).
                    ListOp::Exclusive | ListOp::Forbids => {
                        self.emit_pairwise(&keys, &origin);
                    }
                    // ONEOF: pairwise (at most one) + at-least-one. A ONEOF also
                    // *closes* each of its variables: record every member's object
                    // as a legal value of `(domain, subject, predicate)` so a later
                    // out-of-set reference is caught as a typo (closed-world).
                    ListOp::OneOf => {
                        self.emit_pairwise(&keys, &origin);
                        self.emit_at_least_one(&keys, &origin);
                        for k in &keys {
                            if let (Some(pred), Some(obj)) = (&k.predicate, &k.object) {
                                self.oneof_values
                                    .entry((k.domain.clone(), k.subject.clone(), pred.clone()))
                                    .or_default()
                                    .insert(obj.clone());
                            }
                        }
                    }
                    // ATLEAST: Impossible([NOT a_1, …, NOT a_n]).
                    ListOp::AtLeast => {
                        self.emit_at_least_one(&keys, &origin);
                    }
                }
            }
            Body::Impl {
                antecedent,
                ante_conn,
                consequent,
                cons_conn,
            } => {
                // Implication A → C as `Impossible(A_true ∧ ¬C)`. We group each
                // side by its connective and emit one clause per (ante × cons)
                // group pair — a uniform rule covering all AND/OR combinations:
                //   AND-ante → all its literals share every clause;
                //   OR-ante  → one clause per literal;
                //   AND-cons → one clause per (negated) literal;
                //   OR-cons  → all its (negated) literals share every clause.
                let ante = raw_lits(antecedent, ctx)?;
                let cons = raw_lits(consequent, ctx)?;
                for l in ante.iter().chain(cons.iter()) {
                    self.intern(&l.key);
                }
                let origin = self.origin(source, line, Some(name), kw::PREMISE);

                let ante_groups: Vec<Vec<RawLit>> = match ante_conn {
                    Conn::And => vec![ante.clone()],
                    Conn::Or => ante.iter().map(|l| vec![l.clone()]).collect(),
                };
                let cons_groups: Vec<Vec<RawLit>> = match cons_conn {
                    Conn::And => cons.iter().map(|l| vec![l.clone()]).collect(),
                    Conn::Or => vec![cons.clone()],
                };
                for ag in &ante_groups {
                    for cg in &cons_groups {
                        let mut lits = ag.clone();
                        for c in cg {
                            lits.push(RawLit {
                                key: c.key.clone(),
                                negated: !c.negated,
                            });
                        }
                        self.push_clause(lits, origin.clone());
                    }
                }
            }
            Body::Exists {
                binder,
                domain,
                atom,
            } => {
                // Open (neither SET nor WITNESS): the existential named no candidate,
                // so there is nothing to check. Emit no clause; record a WARNING
                // advisory that nudges the author to name a witness. Never a blow-up
                // — there is nothing to enumerate.
                if let ExistsDomain::Open = domain {
                    let key = ctx.key(&atom.data)?;
                    let origin = self.origin(source, line, Some(name), kw::EXISTS);
                    self.unwitnessed_exists.push(UnwitnessedExists {
                        origin,
                        condition: alloc::format!("{key}"),
                        binder: binder.data.to_string(),
                    });
                    return Ok(());
                }
                // ∃: at least one candidate satisfies the condition. `IN <set>`
                // instantiates the condition per set element; `WITNESS <term>` is
                // the singleton {term} the author names — no SET, exactly one atom.
                // Either way we emit a single at-least-one (an `ATLEAST` whose atoms
                // are generated, not hand-listed); the solver sees nothing new. A
                // witness is `∃` over `{term}`, so there is nothing to enumerate.
                let keys: Vec<AtomKey> = match domain {
                    ExistsDomain::InSet(set) => {
                        let elements = match self.sets.get(set.data) {
                            Some(els) => els.clone(),
                            None => {
                                return Err(CompileError::UnknownSet {
                                    file: source.to_string(),
                                    line: set.span.location_line(),
                                    set: set.data.to_string(),
                                    suggestion: nearest_set_suggestion(set.data, &self.sets),
                                });
                            }
                        };
                        elements
                            .iter()
                            .map(|el| ctx.key(&subst_atom(&atom.data, &[(binder.data, el)])))
                            .collect::<Result<_, _>>()?
                    }
                    ExistsDomain::Witness(w) => {
                        vec![ctx.key(&subst_atom(&atom.data, &[(binder.data, w.data)]))?]
                    }
                    // Handled above with an early return (no clause).
                    ExistsDomain::Open => unreachable!("Open EXISTS emits no clause"),
                };
                for k in &keys {
                    self.intern(k);
                }
                let origin = self.origin(source, line, Some(name), kw::EXISTS);
                self.emit_at_least_one(&keys, &origin);
            }
        }
        Ok(())
    }

    /// Emit "at most one TRUE" as one `Impossible([a_i, a_j])` per unordered
    /// pair. Pairwise (not a single big clause) because `Impossible([a,b,c])`
    /// only forbids *all three* together — it would still allow two.
    fn emit_pairwise(&mut self, keys: &[AtomKey], origin: &Origin) {
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                let lits = vec![
                    RawLit {
                        key: keys[i].clone(),
                        negated: false,
                    },
                    RawLit {
                        key: keys[j].clone(),
                        negated: false,
                    },
                ];
                self.push_clause(lits, origin.clone());
            }
        }
    }

    /// Emit "at least one TRUE" as a single `Impossible([NOT a_1, …, NOT a_n])`
    /// — it is impossible for all of them to be false at once.
    fn emit_at_least_one(&mut self, keys: &[AtomKey], origin: &Origin) {
        let lits = keys
            .iter()
            .map(|k| RawLit {
                key: k.clone(),
                negated: true,
            })
            .collect();
        self.push_clause(lits, origin.clone());
    }

    /// Append a clause unless an identical one (by canonical [`clause_sig`]) is
    /// already present — `P ∧ P ≡ P`, so dedup keeps the IR minimal.
    fn push_clause(&mut self, lits: Vec<RawLit>, origin: Origin) {
        let sig = clause_sig(&lits);
        if self.clause_sigs.insert(sig) {
            self.clauses.push(RawClause { lits, origin });
        }
        // else: identical clause already present — idempotent.
    }

    /// Build an [`Origin`] for provenance from the current source/line/name.
    fn origin(&self, source: &str, line: u32, premise: Option<&str>, kind: &'static str) -> Origin {
        Origin {
            source: source.to_string(),
            line,
            premise: premise.map(|s| s.to_string()),
            kind,
        }
    }

    /// Closed-world check: once an `ONEOF` enumerates a variable's values, any
    /// reference to that `(domain, subject, predicate)` with an object outside the
    /// declared set is rejected as a likely typo — instead of silently minting a
    /// new atom that then "hangs in the air" as an UNKNOWN. Reports the earliest
    /// (by source, line) offender, with a `did you mean` suggestion when a declared
    /// value is within edit distance. Must run after all sources are accumulated
    /// (a `ONEOF` may follow the reference) and before [`finalize`].
    pub(crate) fn validate_closed_world(&self) -> Result<(), CompileError> {
        if self.oneof_values.is_empty() {
            return Ok(());
        }
        // Every atom reference reachable from a fact, clause, or rule, with the
        // line it came from. ONEOF members appear here too (as clause literals) but
        // are in-set by construction, so they never trip the check. `out_of_set`
        // tests one key against its variable's declared values.
        let out_of_set = |key: &AtomKey| -> bool {
            // A bare proposition (predicate `None`) has no object, so it never trips
            // closed-world; only `(predicate, object)` atoms are checked.
            match (key.predicate.as_ref(), key.object.as_ref()) {
                (Some(pred), Some(obj)) => self
                    .oneof_values
                    .get(&(key.domain.clone(), key.subject.clone(), pred.clone()))
                    .is_some_and(|set| !set.contains(obj)),
                _ => false,
            }
        };
        let mut offenders: Vec<(&str, u32, &AtomKey)> = Vec::new();
        for f in &self.facts {
            if out_of_set(&f.key) {
                offenders.push((&f.origin.source, f.origin.line, &f.key));
            }
        }
        for c in &self.clauses {
            for l in &c.lits {
                if out_of_set(&l.key) {
                    offenders.push((&c.origin.source, c.origin.line, &l.key));
                }
            }
        }
        for r in &self.rules {
            for l in r.antecedent.iter().chain(r.consequent.iter()) {
                if out_of_set(&l.key) {
                    offenders.push((&r.origin.source, r.origin.line, &l.key));
                }
            }
        }
        // Earliest offender wins, for a stable, source-ordered diagnostic.
        let Some(&(source, line, key)) = offenders.iter().min_by(|a, b| {
            (a.0, a.1, &a.2.subject, &a.2.object).cmp(&(b.0, b.1, &b.2.subject, &b.2.object))
        }) else {
            return Ok(());
        };
        // The offender tripped `out_of_set`, so its predicate is `Some`.
        let pred = key.predicate.clone().unwrap_or_default();
        let set = &self.oneof_values[&(key.domain.clone(), key.subject.clone(), pred.clone())];
        let declared: Vec<&str> = set.iter().map(|s| s.as_str()).collect(); // BTreeSet → sorted
        let value = key.object.clone().unwrap_or_default();
        let suggestion = did_you_mean(&value, &declared);
        Err(CompileError::UnknownValue(Box::new(UnknownValue {
            file: source.to_string(),
            line,
            subject: key.subject.clone(),
            predicate: pred,
            value,
            declared: declared.join(", "),
            suggestion,
        })))
    }

    /// Resolve every declared `VAR` port against the external `inputs`, validate
    /// that every used bare proposition is declared, and push a synthetic hard fact
    /// for each resolved port. Returns the per-port report records (the PLACEHOLDERS
    /// section). Run after the add-statement loop (all ports declared, all bare
    /// props interned) and before [`finalize`].
    ///
    /// Every ambiguity is a hard error, by design (the engine is deterministic):
    /// - a used bare proposition with no `VAR` → [`CompileError::UndeclaredPort`];
    /// - an external key naming no declared port → [`CompileError::UnknownPort`];
    /// - an external key whose bare name is declared in >1 domain → [`CompileError::AmbiguousPort`];
    /// - two bindings disagreeing on one key → [`CompileError::PortConflict`].
    ///
    /// A resolved port becomes a hard fact (observationally equal to `FACT name` /
    /// `NOT name`); a port with neither value nor `DEFAULT` stays UNKNOWN (no fact).
    pub(crate) fn resolve_ports(
        &mut self,
        inputs: &[(String, PortBinding)],
    ) -> Result<Vec<PlaceholderInfo>, CompileError> {
        // (1) Every *used* bare proposition (in a fact, clause, or rule) must be a
        // declared port. Reported with the earliest source/line, like closed-world.
        {
            let bad = |k: &AtomKey| {
                k.predicate.is_none()
                    && !self
                        .ports
                        .contains_key(&(k.domain.clone(), k.subject.clone()))
            };
            let mut offenders: Vec<(&str, u32, &str)> = Vec::new();
            for f in &self.facts {
                if bad(&f.key) {
                    offenders.push((&f.origin.source, f.origin.line, &f.key.subject));
                }
            }
            for c in &self.clauses {
                for l in &c.lits {
                    if bad(&l.key) {
                        offenders.push((&c.origin.source, c.origin.line, &l.key.subject));
                    }
                }
            }
            for r in &self.rules {
                for l in r.antecedent.iter().chain(r.consequent.iter()) {
                    if bad(&l.key) {
                        offenders.push((&r.origin.source, r.origin.line, &l.key.subject));
                    }
                }
            }
            if let Some(&(source, line, name)) = offenders
                .iter()
                .min_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)))
            {
                let names: Vec<&str> = self.ports.keys().map(|(_, n)| n.as_str()).collect();
                return Err(CompileError::UndeclaredPort {
                    file: source.to_string(),
                    line,
                    name: name.to_string(),
                    suggestion: did_you_mean(name, &names),
                });
            }
        }

        // (2) Resolve every binding's ref (in-file `PROVIDE`s plus external values)
        // to a canonical `AtomKey`, then merge by that key — so a bare and a
        // `domain.`-qualified key for the same target meet in one pool. Two
        // disagreeing values for one resolved key conflict.
        let external = inputs.iter().map(|(k, b)| (parse_port_ref(k), b.clone()));
        let bindings: Vec<(PortRef, PortBinding)> =
            self.provides.iter().cloned().chain(external).collect();
        let mut merged: BTreeMap<AtomKey, PortBinding> = BTreeMap::new();
        for (rf, b) in bindings {
            let key = self.resolve_port_ref(&rf)?;
            match merged.get(&key) {
                Some(prev) if prev.value != b.value => {
                    return Err(CompileError::PortConflict {
                        name: key.to_string(),
                        a_value: prev.value,
                        a_origin: prev.origin.clone(),
                        b_value: b.value,
                        b_origin: b.origin.clone(),
                    });
                }
                _ => {
                    merged.entry(key).or_insert(b);
                }
            }
        }

        // (3) Resolve each declared port: supplied > DEFAULT > unset (UNKNOWN). The
        // placeholder key is bare unless the name collides across domains, in which
        // case it is qualified so the PLACEHOLDERS section stays unambiguous.
        let mut name_counts: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, n) in self.ports.keys() {
            *name_counts.entry(n.as_str()).or_default() += 1;
        }
        let mut placeholders: Vec<PlaceholderInfo> = Vec::new();
        let mut interns: Vec<AtomKey> = Vec::new();
        let mut synth: Vec<(AtomKey, bool, String, u32)> = Vec::new();
        for ((domain, name), decl) in &self.ports {
            let key = AtomKey {
                domain: domain.clone(),
                subject: name.clone(),
                predicate: None,
                object: None,
            };
            interns.push(key.clone());
            let (status, value, origin) = match merged.get(&key) {
                Some(b) => (
                    PlaceholderStatus::Supplied,
                    Some(b.value),
                    Some(b.origin.clone()),
                ),
                None => match decl.default {
                    Some(v) => (PlaceholderStatus::DefaultUsed, Some(v), None),
                    None => (PlaceholderStatus::Unset, None, None),
                },
            };
            if let Some(v) = value {
                synth.push((key, v, decl.source.clone(), decl.line));
            }
            let label = if name_counts.get(name.as_str()).copied().unwrap_or(0) > 1 {
                alloc::format!("{domain}.{name}")
            } else {
                name.clone()
            };
            placeholders.push(PlaceholderInfo {
                key: label,
                status,
                value,
                origin,
            });
        }

        // (4) Intern every declared port (so it appears even if unused), keep it out
        // of the ORPHAN lint, and assert a hard fact for each resolved one.
        for key in interns {
            self.intern(&key);
            self.relation_consumed.insert(key);
        }
        for (key, value, source, line) in synth {
            self.facts.push(RawFact {
                key,
                value: if value { Value::True } else { Value::False },
                origin: Origin {
                    source,
                    line,
                    premise: None,
                    kind: kw::VAR,
                },
                soft: false,
            });
        }

        // (5) A multi-word binding asserts an *atom* (not a VAR port) — inject it as
        // a hard fact, tagged with its supply origin. The atom is already interned
        // (it must occur in the program; `resolve_port_ref` rejects unknown ones).
        for (key, b) in &merged {
            if key.predicate.is_some() {
                self.facts.push(RawFact {
                    key: key.clone(),
                    value: if b.value { Value::True } else { Value::False },
                    origin: Origin {
                        source: b.origin.clone(),
                        line: 0,
                        premise: None,
                        kind: kw::PROVIDE,
                    },
                    soft: false,
                });
            }
        }

        Ok(placeholders)
    }

    /// Resolve one external [`PortRef`] to the canonical [`AtomKey`] it binds.
    /// A single-word ref (`predicate == None`) must match a declared `VAR` port; a
    /// multi-word ref must match an atom the program already uses. A `domain.`
    /// prefix pins the domain; without one, a name found in more than one domain is
    /// [`CompileError::AmbiguousPort`] (resolve by qualifying it).
    fn resolve_port_ref(&self, rf: &PortRef) -> Result<AtomKey, CompileError> {
        if rf.predicate.is_none() {
            let domains: Vec<&str> = self
                .ports
                .keys()
                .filter(|k| k.1 == rf.subject && rf.domain.as_deref().is_none_or(|d| k.0 == d))
                .map(|k| k.0.as_str())
                .collect();
            match domains.as_slice() {
                [] => {
                    let names: Vec<&str> = self.ports.keys().map(|(_, n)| n.as_str()).collect();
                    Err(CompileError::UnknownPort {
                        name: rf.label(),
                        suggestion: did_you_mean(&rf.subject, &names),
                    })
                }
                [d] => Ok(AtomKey {
                    domain: (*d).to_string(),
                    subject: rf.subject.clone(),
                    predicate: None,
                    object: None,
                }),
                many => Err(CompileError::AmbiguousPort {
                    name: rf.subject.clone(),
                    domains: many.join(", "),
                }),
            }
        } else {
            let cands: Vec<&AtomKey> = self
                .keys
                .iter()
                .filter(|k| {
                    k.subject == rf.subject
                        && k.predicate.as_deref() == rf.predicate.as_deref()
                        && k.object.as_deref() == rf.object.as_deref()
                        && rf.domain.as_deref().is_none_or(|d| k.domain == d)
                })
                .collect();
            match cands.as_slice() {
                [] => Err(CompileError::UnknownExternalAtom { name: rf.label() }),
                [k] => Ok((*k).clone()),
                many => Err(CompileError::AmbiguousPort {
                    name: rf.label(),
                    domains: many
                        .iter()
                        .map(|k| k.domain.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                }),
            }
        }
    }

    /// Intern all atoms (canonical sort), then lower the raw IR to ids.
    pub fn finalize(self) -> Compiled {
        let atoms: Vec<AtomKey> = self.keys.into_iter().collect(); // BTreeSet → sorted
        let mut id_of: BTreeMap<AtomKey, AtomId> = BTreeMap::new();
        for (i, k) in atoms.iter().enumerate() {
            id_of.insert(k.clone(), i as AtomId);
        }
        let lower = |l: &RawLit| Lit {
            atom: id_of[&l.key],
            negated: l.negated,
        };

        let facts = self
            .facts
            .into_iter()
            .map(|f| Fact {
                atom: id_of[&f.key],
                value: f.value,
                origin: f.origin,
                soft: f.soft,
            })
            .collect();
        let clauses = self
            .clauses
            .into_iter()
            .map(|c| Clause {
                lits: c.lits.iter().map(lower).collect(),
                origin: c.origin,
            })
            .collect();
        let rules = self
            .rules
            .into_iter()
            .map(|r| Rule {
                antecedent: r.antecedent.iter().map(lower).collect(),
                consequent: r.consequent.iter().map(lower).collect(),
                origin: r.origin,
            })
            .collect();

        let consumed = self
            .relation_consumed
            .iter()
            .filter_map(|k| id_of.get(k).copied())
            .collect();

        let justifications = self
            .justifications
            .into_iter()
            .map(|j| Justification {
                belief: id_of[&j.belief],
                ground: id_of[&j.ground],
                origin: j.origin,
            })
            .collect();

        Compiled {
            atoms,
            facts,
            clauses,
            rules,
            checks: self.checks,
            pending_imports: self.pending_imports,
            unused_imports: Vec::new(), // filled by `compile` (advisory, post-resolution)
            consumed,
            placeholders: Vec::new(), // filled by `*_with` after `resolve_ports`
            unwitnessed_exists: self.unwitnessed_exists,
            justifications,
        }
    }
}
