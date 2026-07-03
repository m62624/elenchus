# ROADMAP — from consistency checker to a formal-logic proof kernel

> Status: **approved 2026-07-04**. This file is the reference for the next arc.
> Integration branch: `roadmap/proof-kernel` — every step of this arc forks from
> and merges back into it; `main` receives the finished arc.

## The mission (north star)

elenchus is a **hybrid**: the **LLM is the prover** (it searches, invents,
instantiates), the **engine is a small trusted kernel** that *verifies* what the
LLM wrote and reports what is missing. The ceiling is **checkability, not
decidability**: checking a supplied witness is cheap and bounded even where
search is undecidable — the same reason Lean's tiny kernel works, with the LLM
replacing the human prover.

The engine today answers *"is all of this consistent together?"*. Formal logic
asks *"does φ follow — and show the derivation"*. What is missing is exactly one
vertical: **entailment (⊨) → checked derivation steps → instantiation of
universals**. Nothing else. Every item below is one bounded check over what was
*written*, never a search over what could be imagined.

## The five laws (every item must prove itself against all five)

1. **Unrepresentable, not checked.** The dangerous case is removed from the
   grammar — it does not parse. Never a lint or a runtime guard.
2. **One binder per construct.** Linearity by construction: a clause can never
   range over the product of two open dimensions.
3. **Cross-products only through declared FACT data.** To relate two objects,
   route through a declared relation; the pairs come from explicit facts.
4. **Compile-time desugar — the solver is never touched.** Features expand to
   ground clauses (or post-verdict checks) via the same emitters; CLI/MCP/wasm
   inherit everything for free through `compile_*`.
5. **The engine checks, never searches.** Whatever cannot be checked in bounded
   cost is rejected by design. Search is the LLM's job.

## The SMT fence — what we never cross (by unrepresentability)

SMT is a move **sideways** (SAT + background theories); formal logic is a move
**up** (quantifiers + proofs). We only move up. Permanently unrepresentable:

- arithmetic and any built-in theory (LIA, bit-vectors, arrays);
- nested function terms `f(g(x))` — the term universe explodes;
- engine-side enumeration of open domains (an unnamed ∀ does not parse);
- any search performed by the engine (Law 5).

## Where we are — the L0–L6 skeleton is built (on `main`, v0.13.0)

| Layer | Science | Engine core | Status |
|---|---|---|---|
| L0 | Propositional three-valued logic | verdicts, CDCL + conflict budget | ✅ done |
| L1 | FOL via witnesses | `EXISTS … WITNESS`, `FOR EACH`, `CLOSE` | ✅ core |
| L2 | Justification calculus (JTB) | `FACT … BECAUSE` | ✅ core |
| L3 | Defeasible / non-monotonic | `RULE … UNLESS` | ✅ core |
| L4 | Belief revision (AGM) | verified `DROP` / `FLIP` fixes | ✅ core |
| L5 | Abduction | `TRY [NOT] <atom>` | ✅ core |
| L6 | Epistemic logic | `KNOWS` / `BELIEVES` | ✅ core |
| infra | — | domains, ports, cross-file relation feeding, ORPHAN lint, diag Cards, deterministic Stats | ✅ |

## Binder & cost summary (the anti-explosion contract)

| Item | Surface form | Binders | Cost |
|---|---|---|---|
| F1 | `PROVE [NOT] <atom>` | **0** | 1 SAT call per line |
| F2 | `HENCE <atom> FROM <p1>, <p2>` | **0** | 1 SAT call per step |
| F3 | universal schema premise | **1** (same as `FOR EACH`) | O(subjects written) |
| F4 | totality check over a relation | **0** | linear scan of declared pairs |
| F5 | `SAME <a> <b>` | **0** | compile-time union-find |
| F6 | `TRY <H> FOR <G>` | **0** | 2 SAT calls per line |
| F7 | `PREFERS` between rules | **0** | comparison of declared pairs |

No item introduces a second binder. F1/F2/F5/F6/F7 have no variables at all;
F4 is a data scan; F3 reuses the existing single binder with a new finite,
compile-time-closed domain.

---

## F1 — The goal: `PROVE [NOT] <atom>` (entailment, ⊨)

**What it adds.** The engine gains the question of formal logic itself:
*does the theory entail φ?* Checked refutationally: `theory ∧ ¬φ` UNSAT →
**PROVED**; `theory ∧ φ` UNSAT → **REFUTED**; both SAT → **OPEN** (the honest
three-valued answer survives). Sibling of `TRY`: TRY asks *compatibility*,
PROVE asks *consequence*.

**LLM supplies:** the goal atom. **Engine checks:** one refutation call.
**Reports:** PROVED / REFUTED / OPEN per goal, post-verdict, advisory
(like TRY/DERIVED — no clause, no verdict change).

**Proof against the laws:**
1. *Unrepresentable:* the form is one already-internable atom; there is no
   production for a compound goal, so nothing unbounded can be asked.
2. *One binder:* zero binders — the goal is ground.
3. *Cross-products:* none — no pairs involved.
4. *Solver untouched:* a post-verdict side-solve on the already-built CNF,
   exactly the `TRY` mechanic (`build_cnf` + a bounded `models` call).
5. *Checks, never searches:* one SAT call on a supplied literal; the engine
   never picks goals itself.

## F2 — Checked derivation: `HENCE <atom> FROM <p1>, <p2>` ⭐ the pivot

**What it adds.** Natural deduction as a *witness language*. The LLM writes a
proof as a chain of steps; the engine verifies each step separately — *do the
named premises entail this conclusion?* — via one refutation call (the F1
machine restricted to the named premises' clauses). The first broken step is
reported by name. Modus ponens, case split, reductio are all instances of the
same single check. This is what turns the checker into a **proof kernel in the
Lean sense**: the prover (LLM) does the creative work, the kernel only re-checks
steps.

**LLM supplies:** the conclusion and the names of the premises/facts it claims
suffice. **Engine checks:** `clauses(p1..pn) ∧ ¬conclusion` UNSAT.
**Reports:** per step — `holds (checked)` / `does not follow — gap here`.

**Proof against the laws:**
1. *Unrepresentable:* references are to *named, already-written* constructs;
   there is no production for referencing anything not in the program, and no
   nesting of HENCE inside HENCE bodies.
2. *One binder:* zero — conclusion and premises are ground names.
3. *Cross-products:* none.
4. *Solver untouched:* per-step side-solve over a *subset* of already-emitted
   clauses; the main solve and its outputs are byte-identical.
5. *Checks, never searches:* the engine never completes a proof or picks
   premises; cost = O(steps written) × one SAT call.

**Open design point:** whether a later step may cite an earlier HENCE
conclusion as a premise (a linear chain — still zero search, still bounded,
cycle forbidden by line order).

## F3 — Universal schema: a premise over "every subject mentioned"

**What it adds.** Today `FOR EACH` ranges only over declared SETs/relations, so
*"all men are mortal"* cannot reach a Socrates the author forgot to enlist. A
schema premise with **one** variable grounds over the set of subjects mentioned
anywhere in the compiled program — finite and closed the moment compilation
ends. (This is the "subject parametrization" need discovered empirically in the
vrf-template-library work.) The domain-naming law is respected: the domain *is*
named — "everything written".

**LLM supplies:** the schema and, implicitly, the individuals (by writing facts
about them). **Engine checks:** the grounded instances, one per subject.
**Reports:** normal clause behavior (this one DOES emit clauses, like FOR EACH).

**Proof against the laws:**
1. *Unrepresentable:* still exactly one variable position per schema — a second
   variable does not parse (same fence as nested `FOR EACH`).
2. *One binder:* by definition; the binder is the existing quantifier shape.
3. *Cross-products:* impossible — one variable cannot form a product; relating
   two subjects still requires a declared relation.
4. *Solver untouched:* grounds at compile time into ordinary clauses through the
   existing emitter.
5. *Checks, never searches:* the ground set is *what was written*, never
   invented; doubling the program at most doubles the instances.

**⚠️ Open design point (the one real decision of the arc):** the domain grows
with the program — importing a file grows it. Options to settle before building:
ground over (a) all subjects in the same domain, (b) all subjects in the whole
merged program, or (c) subjects that match a declared marker fact. Must be
settled so that adding an unrelated file cannot silently change a verdict.

## F4 — Skolem witness tables: `∀x ∃y` meaning, zero new syntax for pairs

**What it adds.** Classical logic writes "every task has an assignee" as a
nested ∀∃. That nesting **does not and will not parse** (Law 1). Instead the
LLM discharges the ∃ **as data** — `FACT task_a assigned bob` — and the engine
performs one flat check: *does every left element of the domain have at least
one declared pair?* Unserved elements are reported by name. The nesting is
unpacked by the canonical move of this codebase: the second dimension lives in
declared FACT pairs.

**LLM supplies:** the witness table (ordinary 3-part facts).
**Engine checks:** totality by a single linear scan.
**Reports:** `total (checked)` / `no witness for: <element>`.

**Proof against the laws:**
1. *Unrepresentable:* the surface names one relation and one already-nameable
   domain of left elements; there is no quantifier syntax at all.
2. *One binder:* zero binders — it is a scan, not a grounding.
3. *Cross-products:* the pairs ARE declared FACT data; this is Law 3 used as
   the feature.
4. *Solver untouched:* a compile-time (or post-verdict, report-side) scan; no
   clause shape changes.
5. *Checks, never searches:* the engine never proposes a witness — it only
   verifies the table the LLM supplied.

## F5 — Identity raised to syntax: `SAME <a> <b>`

**What it adds.** The law of identity (A = A), which today lives silently in
atom interning, becomes writable: two names declared to denote the same thing.
Implemented as compile-time union-find + canonicalization *before* interning.
This is **not** SMT's equality theory: there are no terms, hence no congruence
closure, hence no theory solver — just a rename pass.

**Proof against the laws:**
1. *Unrepresentable:* only two ground atoms; no chained or conditional equality
   parses.
2. *One binder:* zero.
3. *Cross-products:* none.
4. *Solver untouched:* the solver sees only canonical atoms; nothing changes
   after interning.
5. *Checks, never searches:* union-find over written pairs, near-linear.

**Open question (may be cut):** interning already provides identity; F5 pays
off only if real programs need aliasing across files. Decide by usage evidence,
not upfront.

## F6 — Targeted abduction: `TRY <H> FOR <G>`

**What it adds.** The textbook abduction question — *which missing premise
explains G?* — completed. Plain `TRY H` (L5) answers only compatibility;
`TRY H FOR G` accepts H only if (a) `theory + H` stays consistent and
(b) `theory + H ⊨ G` (the F1 machine). Already noted as the clean next step of
L5 when TRY shipped.

**Proof against the laws:**
1. *Unrepresentable:* two ground atoms, no productions for compound H or G.
2. *One binder:* zero.
3. *Cross-products:* none.
4. *Solver untouched:* two side-solves per line, the existing TRY mechanic.
5. *Checks, never searches:* the LLM supplies both H and G; the engine runs
   exactly two bounded checks.

## F7 — Default priorities: `PREFERS` between rules (L3 depth)

**What it adds.** Specificity — *"penguin beats bird"* — as a declared pair
between two named rules. When two defaults clash, the declared preference
resolves it; an undeclared clash keeps today's behavior. The checkable seed of
Dung argumentation: attack edges are *written*, extensions are never computed.

**Proof against the laws:**
1. *Unrepresentable:* names two existing rules; chains/cycles of preference can
   be rejected at compile time like `CLOSE`'s DAG check (cycle = error).
2. *One binder:* zero.
3. *Cross-products:* the preference pairs are declared, never derived.
4. *Solver untouched:* resolved during the existing defeasible gating
   (compile/report side), same slot UNLESS already occupies.
5. *Checks, never searches:* comparison over written pairs only.

## F8 — Epistemic depth (deferred: research)

Nested `K_a K_b φ`, common knowledge, S4/S5 introspection — each needs
possible-world enumeration, which Law 5 forbids the engine to do. Stays behind
the fence until a form is found where the *LLM supplies the world/chain and the
engine only checks it*. Not scheduled.

---

## Build order

**F1 → F2** (the spine: nothing above stands without ⊨; F2 is the pivot to a
proof kernel) → **F3 → F4** (completes the checkable FOL fragment) →
**F5, F6** (cheap, reuse the F1 machine; F5 may be cut) → **F7** → F8 frozen.

After F1+F2 the engine honestly earns the name *formal-logic proof kernel*;
F3+F4 close the checkable first-order fragment.

## Process

- Every F-item: its own branch off `roadmap/proof-kernel`, merged back into it;
  `main` receives the finished arc.
- Every new keyword ships its own diag Card (the local model cannot hit a
  keyword without one).
- Hard gates per merge: `clippy -D warnings`, `fmt --check`, ALL tests + ALL
  snapshots green (byte-identical where a feature is advisory), coverage ≥ 90%
  and not below the running baseline, `search_path_fingerprint_is_stable`
  untouched.
