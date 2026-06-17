# elenchus — a formal reasoning-verification engine

> A small model writes facts and first principles in a simple language.
> A Rust engine does all the logic for it and catches contradictions mathematically.
> The model cannot lie at the inference level — only at the axiom level.
> And a mistake in an axiom we catch early and mechanically.

This document is the specification. It is written so that a person who has never
heard of SAT solvers or three-valued logic can follow it. First — why this is
needed and what was invented before us. Then — how exactly it works. At the very
end — what we deliberately did NOT include and why.

The name *elenchus* (ἔλεγχος) is the Socratic method of cross-examination: you
take someone's claims and interrogate them against first principles until a
contradiction surfaces. That is literally what this engine does to a set of facts.

---

# Part I. Why this is needed at all

## The small-model problem

A local 35B-parameter model (Qwen3 MoE, 8 GB VRAM) classifies and generates code
well. But it has three weaknesses compared to 400B+ models:

1. **Context depth.** It forgets a constraint mentioned at step 2 by the time it reaches step 7.
2. **Calibration.** It does not feel when it is unsure, and confidently hallucinates.
3. **Local minimum.** It gets stuck on the first plausible solution.

"Think harder" does not help here — this is not a matter of effort, it is the
model's ceiling. Typical symptom: the model plans something, says "it works," and
it does not. And you cannot tell why until you check by hand.

## The idea: move the logic out of the model and into the engine

Instead of forcing the model to hold a long chain of reasoning, we split the labor:

```
The LLM is responsible only for FIRST PRINCIPLES (axioms and facts).
All inference and contradiction-finding is done by the ENGINE, not the model.
```

The model writes short statements in a simple language (the DSL). The engine
assembles them into a logical system and checks automatically: consistent / not
consistent / not enough data. The model physically cannot err "in the middle of
the reasoning" — because it is not the one doing the reasoning.

## Honestly: this is not magic

It is worth stating the boundary up front. The model writes the axioms at the
start. If it writes a wrong first principle, the engine will not save it —
garbage in, garbage out. The engine is honest about the *logic of inference*, but
it cannot verify the *truth of an axiom*.

So where is the win? In two things:

1. **Fewer places to err.** Before, the model could err on any of 7 inference
   steps. Now — only in a few axioms. We moved the single point of failure from a
   "long fragile chain" (where the model is weak) to "a few first principles"
   (where the model is strong — it does grasp the essence).
2. **An early mechanical detector.** The model would have made the mistake either
   way. But before, it would surface late (or never). Now any contradiction
   between axioms is caught immediately and for free — the engine simply will not
   let the system converge.

That is, we do not remove the error. We make it **visible early**.

---

# Part II. What was invented before us

We invent almost nothing. All the math is ready, decades (even centuries) old. It
is useful to know whose shoulders we stand on — and why we do not take the most
ambitious path.

| Who | When | What they gave |
|---|---|---|
| **Aristotle** | ~350 BC | The laws of thought: identity, non-contradiction, excluded middle |
| **Leibniz** | ~1690 | The principle of sufficient reason + the `calculemus` dream — "let us compute" instead of arguing |
| **Boole** | 1847 | Boolean algebra: logic as algebra over 0 and 1 |
| **De Morgan** | 1847 | De Morgan's laws — how to rewrite AND/OR/NOT in terms of one another |
| **Sheffer** | 1913 | Proved: a single NAND (`NOT(A AND B)`) suffices for all of logic |
| **Shannon** | 1938 | Connected logic to electrical circuits — hence the whole digital world |
| **Kleene** | 1938 | Three-valued logic: TRUE / FALSE / **UNKNOWN** |
| **Robinson** | 1965 | Resolution — mechanical derivation of contradictions |
| **Davis–Putnam, DPLL** | 1960–62 | The first practical SAT solvers |
| **Prolog / Datalog** | 1972+ | Logic programming (but closed-world: "not stated = false") |
| **minisat, z3** | 2000s | Industrial SAT / SMT solvers the industry runs on |

And there is a path we **deliberately do not take**: the proof assistants **Lean**
and **Coq**. They can prove all of mathematics rigorously. But they pay for it
with monstrously complex languages. That is why Lean did not become the base for
all LLMs — the model cannot write it. Our lesson is direct: **stop in time**. Take
exactly as much logic as is needed to check reasoning, and not a gram more.

---

# Part III. Three things we understood

## What we understood about logic

All of logic reduces to two operations: **NOT + AND**. This set is enough to
express any boolean function — it is a theorem of boolean algebra (and Sheffer
showed back in 1913 that even a single NAND suffices). Shannon in 1938 showed the
adjacent and no less important fact: this same logic *is* electrical circuits, the
hardware itself.

De Morgan's laws show how all other operations follow from NOT+AND:

```
OR(A, B)       = NOT(NOT A AND NOT B)
IMPLIES(A → B) = NOT(A AND NOT B)
XOR(A, B)      = NOT(NOT(A AND NOT B) AND NOT(NOT A AND B))
```

And Aristotle's three laws are not separate rules that must be programmed. In
classical logic they are **theorems** that already live inside NOT+AND. Leibniz
added a fourth — sufficient reason — but that is not a logical law, it is an
epistemic principle ("every truth has a cause").

**Conclusion: there is no need to encode all of science. A single primitive
`Impossible([...])` covers everything.**

> ⚠️ A subtlety worth knowing in advance. We use not classical logic but
> three-valued logic (see below). In it, the law of excluded middle (`A ∨ ¬A`
> always true) **deliberately does NOT hold**: if nothing is known about `A`, then
> `A ∨ ¬A` is also unknown. We keep identity and non-contradiction, but
> deliberately relax excluded middle — that is precisely what gives us the right
> to say "I don't know."

## What we understood about knowledge

The world cannot be split into TRUE and FALSE alone. That is what Prolog does:
"not mentioned = false" (the closed-world assumption). The problem is that the
model is then forced to lie about what it does not know. We need **Kleene's
three-valued logic**:

```
TRUE    — the model explicitly wrote:  FACT A has X
FALSE   — the model explicitly wrote:  NOT  A has X
UNKNOWN — not mentioned at all
```

This is honest. The model writes only what it knows. The engine infers nothing on
its own. The key rule: **UNKNOWN ≠ FALSE**. "I don't know" is a full-fledged third
value, not hidden falsehood.

Notice how this echoes the law of excluded middle above: by allowing UNKNOWN, we
have rejected "either TRUE or FALSE, no third option." There is a third.

## What we understood about verification

A single forward pass is not enough. The engine can say CONSISTENT (no
contradictions), and yet the system is **underdetermined**: a different set of
facts would also pass the check. So a backward pass is also needed — a search for
alternative models.

In total the engine has four outcomes:

```
CONSISTENT       — converges unambiguously, no other variant
UNDERDETERMINED  — converges, but this is not the only possible variant
WARNING          — not enough data to even check
CONFLICT         — a direct contradiction
```

> **WARNING and UNDERDETERMINED are relatives, but of different scale.** WARNING
> is local: one specific axiom could not be checked because of one UNKNOWN.
> UNDERDETERMINED is global: the whole system has more than one solution.
> UNKNOWN facts usually produce both — but WARNING points precisely "it got stuck
> here," while UNDERDETERMINED talks about the system as a whole.

---

# Part IV. Specification

## The single primitive: Impossible

All logical operations reduce to one primitive:

```
Impossible([P1, P2, ..., Pn])
// P1, P2, ..., Pn cannot all be TRUE at the same time
```

`Impossible` is an **internal engine primitive**; the model never writes it. The
model writes CAPS words (`EXCLUSIVE`, `WHEN/THEN`, ...), and the parser reduces
them to `Impossible`.

**Important: `Impossible` accepts negations inside** (`NOT P`). Without that you
cannot express "at least one." A literal = a predicate or its negation.

The surface CAPS words are syntactic sugar over `Impossible`:

```
EXCLUSIVE P Q        =  Impossible([P, Q])           // at most one (pairwise for n>2)
FORBIDS   P Q        =  Impossible([P, Q])           // same as EXCLUSIVE for 2
WHEN P THEN Q        =  Impossible([P, NOT Q])        // implication (body of AXIOM/RULE)
(non-contradiction)  =  Impossible([X, NOT X])        // built in automatically
```

### The sugar is De Morgan's idea — nothing new in the engine

Any boolean constraint reduces to CNF (conjunctive normal form), and a single CNF
clause *is* `Impossible` over literals. So the sugar lives **in the parser**, not
in the engine. The engine never sees the word `ONEOF` — it sees only `Impossible`.
One primitive, many human faces:

```
ATLEAST P Q ...      =  Impossible([NOT P, NOT Q, ...])     // at least one TRUE
ONEOF   P Q ...      =  Impossible([P, Q, ...])             // at most one
                     +  Impossible([NOT P, NOT Q, ...])      // exactly one (= + at least one)
WHEN A AND B THEN C  =  Impossible([A, B, NOT C])           // several conditions
```

Compare with De Morgan — it is literally the same operation:

```
De Morgan:  OR(A, B)   →  NOT(NOT A AND NOT B)
Our sugar:  ATLEAST     →  Impossible([NOT A, NOT B])
```

### How EXCLUSIVE with many elements unfolds — pairwise

A trap: `EXCLUSIVE` = "at most one TRUE," while `Impossible([...])` = "not all
TRUE at once." These are DIFFERENT things. One big clause `Impossible([A, B, C])`
means "not all three" — it allows two to be TRUE. That is NOT "at most one." So
`EXCLUSIVE` unfolds **pairwise**:

```
EXCLUSIVE A B C   →  Impossible([A, B])
                     Impossible([A, C])
                     Impossible([B, C])
```

For n elements this is C(n,2) = n(n−1)/2 clauses of 2 literals each.

**Why pairwise is a win for the CONFLICT pool.** If TRUE turned out to be `A` and
`C`, exactly one clause `Impossible([A, C])` fires → pool = `{A, C}`, precisely who
with whom. The engine names the specific pair, not a vague "axiom violated."

Linear encodings (`commander`/`sequential` with auxiliary variables, O(n) instead
of O(n²)) are needed only for hundreds of elements — but they pollute the pool
with service variables the model never wrote. Our n is small (a handful of
mutually exclusive options) → pairwise is more correct: precise and explainable.

`ONEOF` = pairwise (at most one) + one big clause over negations (at least one):

```
ONEOF A B C   →  Impossible([A, B])
                 Impossible([A, C])
                 Impossible([B, C])
                 Impossible([NOT A, NOT B, NOT C])   // at least one
```

### The varisat → readable-pool bridge

varisat knows only clauses, not axioms. So every generated `Impossible` remembers
its origin: `{from AXIOM <name>, elements i, j}`. When the unsat-core points at a
clause — we translate it back into the axiom name + the specific lines:

```
CONFLICT  fly_xor_swim (EXCLUSIVE)
   FACT Animal.B has flying    [line 3]
   FACT Animal.B has swimming  [line 4]
   → at most one may be TRUE; this pair conflicts
```

### How `WHEN A AND B AND C THEN D` behaves (partial antecedent)

`AXIOM ax: WHEN A AND B AND C THEN D`  =  `Impossible([A, B, C, NOT D])`.

The antecedent `A AND B AND C` is computed by three-valued AND (Kleene): FALSE if
any one is FALSE; TRUE if all three are TRUE; otherwise UNKNOWN. After that the
pair (antecedent, D) decides everything:

| Antecedent | D | Result | Why |
|---|---|---|---|
| FALSE (any of A,B,C = FALSE) | any | CONSISTENT | the rule did not fire, implication vacuously satisfied — this is NOT a warning |
| TRUE (all three TRUE) | TRUE | CONSISTENT | the implication holds |
| TRUE | FALSE | **CONFLICT** | antecedent fired, consequent is false |
| TRUE | UNKNOWN | **WARNING** (AXIOM) / derive D=TRUE (RULE) | consequent undetermined |
| UNKNOWN (no FALSE, some UNKNOWN) | TRUE | CONSISTENT | implication already satisfied, antecedent irrelevant |
| UNKNOWN | FALSE / UNKNOWN | **WARNING** | we don't know whether the rule fires |

The key difference from EXCLUSIVE — **ALL participants land in the CONFLICT pool**,
not a pair:

```
CONFLICT  deploy_rule (WHEN/THEN)
   FACT service tested      = TRUE   [line 3]
   FACT service reviewed    = TRUE   [line 4]
   FACT service has_owner   = TRUE   [line 5]
   NOT  service can_deploy  = FALSE  [line 9]
   → antecedent fired (all TRUE) but consequent is FALSE
```

And the WARNING pool names the specific blocking UNKNOWN literal:

```
WARNING  deploy_rule (WHEN/THEN)
   blocked by: service reviewed = UNKNOWN
   chain: tested=TRUE, has_owner=TRUE, reviewed=UNKNOWN → antecedent UNKNOWN → STOP
   hint: add  FACT service reviewed   OR   NOT service reviewed
```

### `AXIOM` (checks) vs `RULE` (derives) — two modes of one implication

The body of both is identical — `WHEN ... THEN ...` = `A => B` = `Impossible([A, NOT B])`.
The difference is operational, set by the leading keyword:

```
AXIOM ...  WHEN A THEN B   — CHECKS.   A=TRUE, B=FALSE → CONFLICT
RULE  ...  WHEN A THEN B   — DERIVES.  A=TRUE → ADDS fact B
```

One **checks**, the other **produces** a new fact. This distinction is important to keep in mind.

## DSL: keywords

**v1 — a purely boolean system.** The core is 5 concepts (`FACT`, `NOT`, `AXIOM`,
`RULE`, `CHECK`), plus a few words for the body of constraints and rules, plus
`IMPORT` for reuse.

| Word | Meaning | Kind |
|---|---|---|
| `FACT` | a TRUE assertion | axiom (unchecked) |
| `NOT` | a FALSE assertion | axiom (unchecked) |
| `AXIOM` | a first principle — **checked** | constraint |
| `RULE` | an inference rule — **produces a fact** | rule, forward chaining |
| `WHEN` / `AND` / `THEN` | implication body (in `AXIOM` and `RULE`) | |
| `EXCLUSIVE` / `FORBIDS` / `ONEOF` / `ATLEAST` | list constraints (in `AXIOM`) | |
| `IMPORT` | pull in another source for reuse | reuse |
| `CHECK` / `BIDIRECTIONAL` | a query | query |

### Case convention (mandatory)

**Keywords are ALWAYS CAPS, like in SQL.** Names of subjects, predicates and rules
are lowercase. This gives a visual separation: CAPS = grammar, lowercase =
content. The model does not confuse a keyword with an atom name, and a human
instantly sees the structure in a long sequence.

Comments: `//`. In v1 there are NO numbers — an atom like `has flying` is only
TRUE / FALSE / UNKNOWN.

## DSL syntax

Principle: **one line — one statement, no nested parentheses, no operator
precedence.** Short forms (`FACT`/`NOT`/`CHECK`/`IMPORT`) — on one line. Compound
forms (`AXIOM`/`RULE`) — as a vertical block with indentation, each condition on
its own line.

```
// Reuse: pull facts/axioms/rules from another source
IMPORT "physics.vrf"

// Facts (TRUE) and negations (FALSE) — boolean atoms, no numbers, one line
FACT <Subject> <predicate> [<object>]
NOT  <Subject> <predicate> [<object>]

// List axiom: EXCLUSIVE / FORBIDS / ONEOF / ATLEAST
AXIOM <name>:
    EXCLUSIVE
        <Subject> <predicate> [<object>]
        <Subject> <predicate> [<object>]

// Implication axiom — CHECKED (violation → CONFLICT)
AXIOM <name>:
    WHEN <Subject> <predicate>
    AND  <Subject> <predicate>
    THEN <Subject> <predicate>

// Rule — DERIVES a new fact (forward chaining)
RULE <name>:
    WHEN <Subject> <predicate>
    AND  <Subject> <predicate>
    THEN <Subject> <predicate>

// Query
CHECK <Subject>
CHECK <Subject> BIDIRECTIONAL    // enables the backward pass
```

The difference between `AXIOM` and `RULE` with an identical WHEN/THEN body:
`AXIOM` **checks** (no convergence → CONFLICT), `RULE` **produces** a new fact.

## IMPORT — reuse over a source-agnostic engine

The engine is **source-agnostic: it consumes strings.** A file is merely one way
to reuse a body of facts and axioms. Resolution goes through a `Resolver`
abstraction (mirroring vsm-grammar's `SourceResolver` / `MemoryResolver` /
`FileResolver`): `IMPORT "physics.vrf"` asks the resolver for the string named
`physics.vrf`, however it is stored (file, in-memory map, network).

This is the killer feature for a small model. The whole thesis is "the model can
only err at the axiom level." A vetted axiom library turns this into "the model
**cannot** err at the axiom level at all" — it writes only `FACT` lines and pulls
the first principles from a curated, human-reviewed library. It also saves the
small model's context: its generated file is just facts.

### Semantics: flat merge into one shared atom universe

`IMPORT` performs a **flat merge** of all `FACT` / `NOT` / `AXIOM` / `RULE` from
the imported source into the current set. Crucially, **atoms unify across sources
by identity** — an imported axiom about `Engine.X has fuel` constrains the fact
`Engine.X has fuel` declared in the main file. There is **no alias namespacing of
atoms** (that would break unification, which is the whole point of importing
axioms). Atoms live in one global namespace keyed by `(subject, predicate, object?)`.

### Duplicate axioms are idempotent — not a conflict

Importing the same library twice, or two libraries sharing a lemma, is
automatically harmless. The engine compiles everything to a **set** of
`Impossible([...])` clauses (CNF); feeding an identical clause to the solver is a
no-op, because `P ∧ P ≡ P`. No special handling is needed at the logic level.

Two bookkeeping guards exist on top, and content-addressing (sha256, mirroring
vsm-guard's CAS) is the natural tool for both:

- **Dedup for reports.** The same axiom is not listed twice in a conflict pool;
  identical normalized content → identical content hash → one first principle.
- **Redefinition is an error.** The same axiom **name** with a **different** body
  is a genuine `AxiomRedefinition` error, caught by comparing content hashes under
  one name.

### Cycle detection and dedup of sources

`IMPORT` cycles (`a` imports `b` imports `a`) and repeated imports are handled
exactly like vsm's `VstCompiler`: each resolved source is hashed (sha256), a
`visit_stack` of hashes detects circular dependencies, and an already-compiled
hash is reused rather than re-parsed.

> Note: the sha256 content-addressing is used **only** for source/axiom dedup,
> integrity, and provenance — **never** for namespacing atoms. Atoms get a flat,
> deterministic interner instead (see Two passes / implementation).

## Grammar (EBNF)

**Indentation is cosmetic, NOT significant.** Block boundaries are determined by
keywords, not by indentation depth or tabs. This removes the classic LLM
whitespace error. It works thanks to the CAPS convention: a statement line always
starts with a CAPS keyword, and an atom line starts with a lowercase identifier.
The parser does not confuse them.

```ebnf
program     = { line } ;
line        = comment | blank | statement ;
comment     = "//" , { any-char-except-newline } , NEWLINE ;
blank       = NEWLINE ;

statement   = import | fact | negation | axiom | rule | check ;

import      = "IMPORT" , string , NEWLINE ;
fact        = "FACT" , atom , NEWLINE ;
negation    = "NOT"  , atom , NEWLINE ;
check       = "CHECK" , [ subject ] , [ "BIDIRECTIONAL" ] , NEWLINE ;

axiom       = "AXIOM" , name , ":" , NEWLINE , ( list_body | impl_body ) ;
rule        = "RULE"  , name , ":" , NEWLINE , impl_body ;

list_body   = list_op , NEWLINE , atom_line , atom_line , { atom_line } ;  (* >= 2 *)
list_op     = "EXCLUSIVE" | "FORBIDS" | "ONEOF" | "ATLEAST" ;
atom_line   = atom , NEWLINE ;

impl_body   = when_line , { and_line } , then_line , { and_line } ;
when_line   = "WHEN" , literal , NEWLINE ;
then_line   = "THEN" , literal , NEWLINE ;
and_line    = "AND"  , literal , NEWLINE ;

atom        = subject , predicate , [ object ] ;
literal     = [ "NOT" ] , atom ;
subject     = identifier ;
predicate   = identifier ;
object      = identifier ;
name        = identifier ;
string      = '"' , { any-char-except-quote } , '"' ;

identifier  = letter , { letter | digit | "_" | "." } ;
letter      = "A".."Z" | "a".."z" ;
digit       = "0".."9" ;
```

How the parser finds the end of an `AXIOM`/`RULE` block: the block continues while
lines start with body words (`WHEN`/`AND`/`THEN` or a `list_op`) or with an
identifier (list atoms), and ends at the first line with a top-level word
(`IMPORT`/`FACT`/`NOT`/`AXIOM`/`RULE`/`CHECK`) or at EOF. An `AND` before `THEN` is
an antecedent condition; an `AND` after `THEN` is an additional consequent.

Reserved words (always CAPS, in full): `IMPORT FACT NOT AXIOM RULE CHECK
BIDIRECTIONAL WHEN AND THEN EXCLUSIVE FORBIDS ONEOF ATLEAST`. An identifier may not
coincide with a reserved word.

## Name normalization

Silent splitting of atoms (`has_fuel` in one place, `hasFuel` in another) is the
main hidden risk. So the identity rules are fixed hard:

1. **Keywords** — UPPERCASE only, compared exactly (`FACT`, not `Fact`).
2. **Identifiers are case-sensitive and compared verbatim.**
   `has_fuel` ≠ `hasFuel` ≠ `Has_fuel` — these are DIFFERENT atoms. No auto-matching.
3. **Whitespace**: runs of spaces/tabs collapse to one separator; leading and
   trailing are trimmed. `Creature.A   has    flying` ≡ `Creature.A has flying`.
4. **Atom identity** = `(subject, predicate, object?)` character by character,
   case-sensitive, after whitespace normalization.
5. **Comments** `//` to end of line are stripped before parsing. Blank lines are ignored.
6. **Convention (not enforced)**: `snake_case` for predicates/objects, `Subject`
   or `Type.Instance` for subjects. This reduces typo risk, but the engine does
   not enforce it.

> Because identity is verbatim, a typo or a different spelling silently creates a
> NEW atom — it lands in UNKNOWN, and the engine does not know the two names mean
> the same thing. This is residual discipline on the model. (A future soft hint:
> warn about near-duplicates by edit distance — but that is a heuristic, not v1.)

## How code tasks map into boolean logic

The most common question: "but what about numbers? code has `if speed >= 100`."
Answer: **a number becomes a named boolean atom**. The number 100 itself stays in
the code, and the engine reasons only about "branch taken or not."

```
// in code:  if motor.speed >= 100 { fast_path() } else { slow_path() }

// in .vrf — the condition becomes a boolean atom:
FACT Motor over_100              // TRUE — this condition holds

RULE pick_fast:
    WHEN Motor over_100
    THEN Motor uses fast_path

RULE pick_slow:
    WHEN NOT Motor over_100
    THEN Motor uses slow_path

AXIOM one_path:
    EXCLUSIVE
        Motor uses fast_path
        Motor uses slow_path
```

The engine does not know what "100" or "km/h" is. It knows only: the atom
`Motor over_100` is TRUE, FALSE or UNKNOWN. And it checks the **branching logic**,
not the arithmetic:

```
✓ both branches handled?   — if only fast_path exists → WARNING about slow_path
✓ branches don't conflict? — EXCLUSIVE catches "both at once"
✓ branch reachable?        — if over_100 = FALSE everywhere → fast_path is dead code
```

If there are several thresholds (`>= 100`, `>= 200`) — each is its own atom, and
their numeric order is expressed by an **axiom**, with no arithmetic at all:

```
AXIOM speed_order:
    WHEN Motor over_200
    THEN Motor over_100
// if speed > 200, then it is NECESSARILY > 100 — pure logic
```

So the engine catches the bug "you handled >200 as the slow path, even though >200
also means >100" — all without a single number.

## The engine's two passes

### Forward pass

```
facts + axioms → saturate with rules → check constraints → result
```

Phases:
1. **Parse** — tokenize DSL → AST
2. **Bind** — build `FactStore: Map<Atom, (Value, Source)>`, where
   `Atom = (subject, predicate, object?)` in full (see the atom-identity invariant)
3. **Saturate** — apply `RULE` rules to a fixpoint; on each step check non-contradiction
4. **Verify** — for each `AXIOM`: evaluate(constraint, fact_store)
5. **Report** — collect results with provenance (where each fact came from)

### Backward pass (model finding)

Triggered by `CHECK X BIDIRECTIONAL`, or always on `CHECK X` in strict mode.

```
take the current fact set → search for an alternative fact set
that also satisfies all axioms → if found → UNDERDETERMINED
```

Example:
```
FACT A has flying
AXIOM ax:
    EXCLUSIVE
        A has flying
        A has swimming

Forward:  CONSISTENT
Backward: A has swimming also satisfies all axioms
→ UNDERDETERMINED: an alternative model exists
  hint: add  NOT A has swimming  to pin it down unambiguously
```

## The engine's four results

```
CONSISTENT       — no contradictions, a unique model
UNDERDETERMINED  — the logic does not break, but there is an alternative interpretation
CONFLICT         — an axiom is violated, a contradiction was found
WARNING          — not enough data to check (UNKNOWN in a critical place)
```

### Output format

```
CHECK: <Subject>
  [CONSISTENT|UNDERDETERMINED|CONFLICT|WARNING]  <axiom>  <details>
  ...
SUMMARY: <n> conflicts, <m> underdetermined, <k> warnings, <j> consistent
EXIT_CODE: 0=consistent, 1=underdetermined/warnings, 2=conflicts
```

### Example of full output

```
CHECK: Creature.A BIDIRECTIONAL
  CONSISTENT     no_dual_temp        — has cold_blood = UNKNOWN, no conflict
  WARNING        wings_need_bone     — UNKNOWN: has wing, has bone
                 hint: FACT Creature.A has wing
                       FACT Creature.A has bone
                  OR   NOT  Creature.A has flying
  UNDERDETERMINED fly_xor_swim       — alternative model: has swimming=TRUE, has flying=FALSE
                 hint: NOT Creature.A has swimming  to pin it down unambiguously
  DERIVED        Creature.A needs oxygen — from RULE [line 9] + FACT has flying [line 3]

SUMMARY: 0 conflicts, 1 underdetermined, 1 warning, 1 consistent, 1 derived
EXIT_CODE: 1
```

## UNKNOWN, WARNING and three-valued forward chaining

This is the heart of the system. The engine aims at **simple logic**: it
immediately shows what converges, and honestly shows what cannot be computed —
because an UNKNOWN component participates in the needed inference.

### Confident vs blocked

```
confident fact   — TRUE or FALSE. Participates in inference, gives a result.
blocked          — an UNKNOWN participates in the inference. The result would be UNKNOWN → stop.
```

Confident facts produce results, those results feed other results — the chain goes
deep until it reaches the needed point. But if a branch hits even one UNKNOWN —
**we do not go deeper** (no point, the result would be UNKNOWN anyway). We log it
and take the next combination.

### Algorithm (three-valued forward chaining)

```
1. Start: only confident facts (TRUE/FALSE)
2. For each rule/axiom compute the antecedent by three-valued AND (Kleene):
      - any input FALSE   → antecedent FALSE → the rule does NOT fire,
                            implication vacuously satisfied → CONSISTENT, NO warning
      - all inputs TRUE   → antecedent TRUE  → go to THEN (step 3)
      - otherwise (some UNKNOWN, no FALSE) → antecedent UNKNOWN → branch blocked (step 4)
3. Antecedent TRUE:
      RULE:  derive the THEN fact = TRUE (if it is already FALSE → CONFLICT)
      AXIOM: check THEN — TRUE→CONSISTENT, FALSE→CONFLICT, UNKNOWN→WARNING
4. Blocked branch (antecedent UNKNOWN):
      - if THEN is already TRUE → implication satisfied → CONSISTENT (no warning)
      - otherwise → WARNING, blocked by the specific UNKNOWN literal; do not go deeper
5. Repeat to a fixpoint (newly derived facts trigger other rules)
6. Finish: successful (consistent) branches + the whole WARNING pool with branches
```

The key: **a confidently-FALSE input short-circuits the branch to CONSISTENT, not
WARNING.** A warning arises only when a branch is genuinely "alive" but hits an UNKNOWN.

### The three pools in the output

| Pool | What it collects | Source |
|---|---|---|
| **CONFLICT pool** | which facts+axioms together produce a contradiction | unsat-core from SAT (or computed ourselves) |
| **WARNING pool** | which branches are blocked and by which UNKNOWN component | our layer, during forward chaining |
| **CONSISTENT** | can be hidden; or list all successful combinations | our layer |

CONFLICT immediately shows its pool — who conflicts with whom. WARNING by analogy
shows its pool — where and because of what it got stuck. CONSISTENT may stay
silent or list successful combinations for clarity.

### Example WARNING pool

```
WARNING pool (2 blocked branches):
  [branch 1]  AXIOM wings_need_bone
              blocked by: has wing = UNKNOWN
              chain: FACT has flying → WHEN has flying THEN has wing AND has bone → STOP at has wing
  [branch 2]  AXIOM needs_fuel
              blocked by: has engine = UNKNOWN
              chain: RULE WHEN has engine THEN needs fuel → STOP at has engine

  hint: add  FACT ... has wing  and  FACT ... has engine
        to unblock both branches
```

So the model sees not just "not enough data," but **precisely where and in which
chain** it got stuck — and exactly what to add.

## Invariants and edge cases

This is what the engine must guarantee ALWAYS. The spec is not considered complete
until every point is covered.

### Atom identity
An atom is the triple `(subject, predicate, object?)`, object optional.
`Creature.A has flying` and `Creature.A has swimming` are DIFFERENT atoms (the
object differs). Two atoms are equal ⇔ all three parts match character by
character (after whitespace normalization). The FactStore key is exactly this
whole triple, not `(subject, predicate)`.

### Non-contradiction at the fact level
`FACT X` and `NOT X` at once → immediate CONFLICT (an atom cannot be TRUE and
FALSE at the same time). A duplicate (`FACT X` twice) is idempotent, not an error.

### Derived facts
A `RULE` that derived `D = TRUE`, with `NOT D` present → CONFLICT (through the same
non-contradiction mechanism during saturate).

### Saturate termination
Rules are monotone (they only add facts), the atom set is closed — the engine does
not invent new names, only those written in the file. So saturation reaches a
fixpoint in a finite number of steps. A cycle (`A⇒B`, `B⇒A`) is safe: both become
TRUE and the process halts. A contradiction-cycle (`A⇒B`, `B⇒NOT A`) is caught by
non-contradiction.

### Conflict among the axioms themselves
Axioms can be incompatible even without facts (e.g. they mutually require
exclusive things). SAT will catch this: the CONFLICT pool then names axioms, not facts.

### Duplicate axioms across imports
Identical constraints (same content hash) are idempotent — merged into one clause,
never a conflict (`P ∧ P ≡ P`). The same axiom name with a different body is an
`AxiomRedefinition` error. See the IMPORT section.

### CHECK scope
`CHECK <Subject>` reports on axioms and rules where this subject participates.
`CHECK` without a subject — on the whole system. A subject with no facts at all →
almost everything is WARNING.

### Enumeration bound in BIDIRECTIONAL
Alternative models are found by enumerating over UNKNOWN atoms — up to 2^k
variants. The enumeration is capped by a configurable limit. On exceeding it the
engine reports "≥ N alternative models" and shows the first divergence, rather than hanging.

### Output determinism
For the same input the output is byte-for-byte identical: results are sorted
(CONFLICT → WARNING → UNDERDETERMINED → CONSISTENT; within — by line number). This
is needed so the model and diff tools see stable text. Atom interning is canonical
(sorted) so variable ids and any enumeration order are deterministic too.

### Literals in WHEN / AND / THEN
In these positions a literal is allowed — a predicate or its negation (`NOT ...`).
`THEN NOT X` means: for an `AXIOM` — X must be FALSE; for a `RULE` — derive X=FALSE.

---

# Part V. Implementation

## What we take ready-made, what we write ourselves

We write no heavy math. Everything of our own is the parser, the gluing, and the formatting.

| Layer | How it is covered |
|---|---|
| `.vrf` parser | written by us (`nom` + `nom_locate`, `no_std`) — crate `elenchus-parser` |
| Import resolution, desugaring, atom interning, content-addressing | written by us — crate `elenchus-compiler` |
| Boolean logic, CONFLICT / CONSISTENT | a no_std port of `varisat` — future crate `elenchus-solver` |
| Backward pass / UNDERDETERMINED | `varisat` in all-SAT mode (enumerate all models) |
| Three-valuedness (UNKNOWN not passed to SAT) | a thin layer, written by us |
| WARNING pool, provenance, hints | our layer, written by us |
| Output formatting | ourselves |

### Crate layout (mirrors the vsm workspace)

```
elenchus/
  crates/
    elenchus-parser/    no_std; text → AST; nom + nom_locate; Span/Located/pretty errors
    elenchus-compiler/  no_std + std; AST → canonical Impossible/CNF clause IR;
                        Resolver-based IMPORT, desugaring, atom interner, sha256 CAS
    elenchus-solver/    (future) no_std port of varisat; std + multithread features
```

Dependency versions are pinned 1:1 with the vsm workspace (`nom` 8, `nom_locate`
5, `thiserror` 2, `sha2` 0.10). The parser is our own (English-like, not
S-expressions), but in vsm-parser's style: zero-copy over `&str`, `Span`/`Located`
for line/column tracking, and a human-friendly `^--- here` error display.

> **Honestly about varisat.** A SAT solver knows only TRUE/FALSE. It knows nothing
> about UNKNOWN or about WARNING — we carry the whole WARNING pool ourselves on top
> of it. Alternative models (UNDERDETERMINED) it can do: it finds a solution, adds
> a blocking clause, solves again — so all variants are enumerated (all-SAT). The
> single bottleneck: before passing to SAT we must convert values **cleanly** —
> TRUE → true, FALSE → false, and UNKNOWN must NOT be passed, but collected
> separately. Passing UNKNOWN as false → false CONFLICTs; as true → false
> CONSISTENTs. This is the one place where you must be precise.

## Project goal: phase 1 only

**We build one thing — the boolean engine (phase 1).** It checks structural
consistency: contradictions, cycles, missing premises, underdetermination. This is
exactly what the small model loses on a long chain of reasoning.

Arithmetic and computation the engine does NOT do — those are done by the
generated code itself and by tests. For the task "help the model plan and write
code," this is enough.

## Integration (plan)

1. Standalone Rust CLI: `elenchus check file.vrf`
2. JSON output: `elenchus check file.vrf --format json`
3. An npm wrapper → a `planner_verify` tool in pi-code-planner
4. Insertion point in the state machine: an `execution/contract_check` step
   or a new `verify_reasoning` step between discovery and planning

elenchus is an independent project; integration into pi-code-planner is a later,
optional step via the npm wrapper.

---

# Part VI. What is NOT included and why

Here is everything we deliberately left out, and what complexity it removed for the
model. Each point is a place where we consciously stopped, so as not to repeat
Lean's fate.

| What we excluded | Why | What it would mean for the model |
|---|---|---|
| **Numbers and arithmetic** (`speed = 100`, `derive`) | `Impossible` works over boolean values, not numbers. That is SMT, a different engine. | The model would have to write formulas and computation chains — exactly what it errs at most. Instead: number → boolean atom. |
| **Quantifiers** (∀x, ∃x — "for all," "exists") | They sharply complicate both the parser and the solver. | The model would get confused about variable scope. For checking concrete facts they are not needed. |
| **Probabilities** | That is different math (Bayesian), not boolean logic. | Mixing "probable" and "true" is a source of calibration errors — exactly the model's weakness. |
| **Nested axioms** | Axioms are always flat. | Nesting = deep structure the model holds poorly. A flat list is easier to write and read. |
| **Types and entity schemas** | No `type`, `class`, schema declarations. | Extra ceremony before writing a fact. The model writes a fact immediately, with no preamble. |

## Phase 2 (optional, most likely not needed): z3

If one day it becomes necessary to prove **numeric** statements without running
code (`x > 0 ⇒ x + 1 > 1` for all x) — there is a ready SMT solver **z3**
(open-source, MIT license, Microsoft Research; Rust crate `z3-sys`). No math from
scratch would be needed — z3 *is* the ready "math system," and its input language
SMT-LIB is something like "LaTeX for computation."

But for coding it is almost certainly overkill:

- z3's language (SMT-LIB) — Lisp-like parentheses, **hard for small models**;
- z3 is a heavy native C++ dependency, not pure Rust;
- arithmetic in code is already checked by **the compiler and tests** — no point duplicating.

So z3 stays a theoretical "later" possibility, not a planned phase.

### Untangling the "you can't prove it without computing" confusion

These are two independent things that are easy to confuse:

```
Structurally consistent?  → phase 1 (boolean form, are there contradictions)
Numerically correct?      → code + tests (or z3, if really needed)
```

Phase 1 catches a huge class of errors WITHOUT a single number. You do not need to
"compute the whole world" for the engine to be useful — it is about logic, not numbers.

---

# Appendix. Example `.vrf` file

```vrf
// Investigating creature A on an unknown planet.
// One explorer says it flies, another says it swims.
// The book says: one of the two, not both at once.

FACT Creature.A has flying
FACT Creature.A has warm_blood
// has swimming not mentioned → UNKNOWN

// First principles
AXIOM fly_xor_swim:
    EXCLUSIVE
        Creature.A has flying
        Creature.A has swimming

AXIOM wings_need_bone:
    WHEN Creature.A has flying
    THEN Creature.A has wing
    AND  Creature.A has bone

AXIOM no_dual_temp:
    FORBIDS
        Creature.A has warm_blood
        Creature.A has cold_blood

// Inference rule — produces a new fact
RULE needs_oxygen:
    WHEN Creature.A has flying
    THEN Creature.A needs oxygen

CHECK Creature.A BIDIRECTIONAL
```
