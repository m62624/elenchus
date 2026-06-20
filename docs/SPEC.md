# elenchus — a simplified SAT checker for small local LLMs

> A small model writes facts and first principles in a simple language; a Rust
> engine does the boolean bookkeeping and flags contradictions. The model can only
> get a premise wrong — never a step in a long chain — and that is caught mechanically.
>
> In one line: a small SAT checker with three-valued logic (TRUE / FALSE /
> UNKNOWN), aimed at small local models. Nothing more.

This document is the specification. It is written so that someone who has never
heard of SAT solvers or three-valued logic can follow it: first why it exists,
then how it works, and at the end what it leaves out.

The name comes from *elenchus* (ἔλεγχος) — Socratic refutation by finding
contradictions. That is just the spirit of it; mechanically this is a small
consistency/SAT checker, not a dialogue.

---

# Part I. Why this is needed at all

## The small-model problem

A small local model classifies and generates code well. But compared to the big
frontier models it has three weaknesses:

1. **Context depth.** It forgets a constraint mentioned at step 2 by the time it reaches step 7.
2. **Calibration.** It does not feel when it is unsure, and confidently hallucinates.
3. **Local minimum.** It gets stuck on the first plausible solution.

"Think harder" does not help here — this is not a matter of effort, it is the
model's ceiling. Typical symptom: the model plans something, says "it works," and
it does not. And you cannot tell why until you check by hand.

## The idea: move the logic out of the model and into the engine

Instead of forcing the model to hold a long chain of reasoning, we split the labor:

```
The LLM is responsible only for FIRST PRINCIPLES (premises and facts).
All inference and contradiction-finding is done by the ENGINE, not the model.
```

The model writes short statements in a simple language (the DSL). The engine
assembles them into a logical system and checks automatically: consistent / not
consistent / not enough data. The model physically cannot err "in the middle of
the reasoning" — because it is not the one doing the reasoning.

## Limits — this is not magic

It is worth stating the boundary up front. The model writes the premises at the
start. If it writes a wrong first principle, the engine will not save it —
garbage in, garbage out. The engine is honest about the *logic of inference*, but
it cannot verify the *truth of a premise*.

So where is the win? In two things:

1. **Fewer places to err.** Before, the model could err on any of 7 inference
   steps. Now — only in a few premises. We moved the single point of failure from a
   "long fragile chain" (where the model is weak) to "a few first principles"
   (where the model is strong — it does grasp the essence).
2. **An early mechanical detector.** The model would have made the mistake either
   way. But before, it would surface late (or never). Now any contradiction
   between premises is caught immediately and for free — the engine simply will not
   let the system converge.

That is, we do not remove the error. We make it **visible early**.

---

# Part II. The core ideas

## Logic

All of logic reduces to two operations: **NOT + AND**. That is enough to express
any boolean function (a standard result of boolean algebra).

De Morgan's laws show how all other operations follow from NOT+AND:

```
OR(A, B)       = NOT(NOT A AND NOT B)
IMPLIES(A → B) = NOT(A AND NOT B)
XOR(A, B)      = NOT(NOT(A AND NOT B) AND NOT(NOT A AND B))
```

So we don't encode many rules. A single primitive — `Impossible([...])` —
expressed in NOT+AND form is enough for everything the checker does.

> ⚠️ A subtlety worth knowing in advance. We use not classical logic but
> three-valued logic (see below). In it, the law of excluded middle (`A ∨ ¬A`
> always true) **deliberately does NOT hold**: if nothing is known about `A`, then
> `A ∨ ¬A` is also unknown. We keep identity and non-contradiction, but
> deliberately relax excluded middle — that is precisely what gives us the right
> to say "I don't know."

## Knowledge

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

## Verification

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
> is local: one specific premise could not be checked because of one UNKNOWN.
> UNDERDETERMINED is global: the whole system has more than one solution.
> UNKNOWN facts usually produce both — but WARNING points precisely "it got stuck
> here," while UNDERDETERMINED talks about the system as a whole.

---

# Part III. Specification

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
WHEN P THEN Q        =  Impossible([P, NOT Q])        // implication (body of PREMISE/RULE)
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
WHEN A THEN C OR D   =  Impossible([A, NOT C, NOT D])       // disjunctive consequent
WHEN A OR B THEN C   =  Impossible([A, NOT C])              // disjunctive antecedent
                     +  Impossible([B, NOT C])              //   = (A→C) ∧ (B→C)
```

`AND`/`OR` may appear in either the `WHEN` group or the `THEN` group, but one
group may not mix them (`WHEN a AND b OR c` is a parse error — split it). The four
combinations follow one rule: group each side by its connective, then emit one
`Impossible(antecedent-group ++ ¬consequent-group)` per group pair (AND-ante puts
all its literals in every clause; OR-ante makes one clause per literal; AND-cons
makes one clause per literal; OR-cons puts all its literals in every clause). A
`RULE` *derives* its consequent, so `OR` in a `RULE`'s `THEN` is rejected (a rule
cannot assert a disjunction) — use a `PREMISE` constraint instead.

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
with whom. The engine names the specific pair, not a vague "premise violated."

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

varisat knows only clauses, not premises. So every generated `Impossible` remembers
its origin: `{from PREMISE <name>, elements i, j}`. When the unsat-core points at a
clause — we translate it back into the premise name + the specific lines:

```
CONFLICT  fly_xor_swim (EXCLUSIVE)
   FACT Animal.B has flying    [line 3]
   FACT Animal.B has swimming  [line 4]
   → at most one may be TRUE; this pair conflicts
```

### How `WHEN A AND B AND C THEN D` behaves (partial antecedent)

`PREMISE ax: WHEN A AND B AND C THEN D`  =  `Impossible([A, B, C, NOT D])`.

The antecedent `A AND B AND C` is computed by three-valued AND (Kleene): FALSE if
any one is FALSE; TRUE if all three are TRUE; otherwise UNKNOWN. After that the
pair (antecedent, D) decides everything:

| Antecedent | D | Result | Why |
|---|---|---|---|
| FALSE (any of A,B,C = FALSE) | any | CONSISTENT | the rule did not fire, implication vacuously satisfied — this is NOT a warning |
| TRUE (all three TRUE) | TRUE | CONSISTENT | the implication holds |
| TRUE | FALSE | **CONFLICT** | antecedent fired, consequent is false |
| TRUE | UNKNOWN | **WARNING** (PREMISE) / derive D=TRUE (RULE) | consequent undetermined |
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

### `PREMISE` (checks) vs `RULE` (derives) — two modes of one implication

The body of both is identical — `WHEN ... THEN ...` = `A => B` = `Impossible([A, NOT B])`.
The difference is operational, set by the leading keyword:

```
PREMISE ...  WHEN A THEN B   — CHECKS.   A=TRUE, B=FALSE → CONFLICT
RULE  ...  WHEN A THEN B   — DERIVES.  A=TRUE → ADDS fact B
```

One **checks**, the other **produces** a new fact. This distinction is important to keep in mind.

## `ASSUME` — soft, retractable hypotheses

`FACT`/`NOT` are commitments; `ASSUME` is a **hypothesis**. Syntactically it is a
fact that accepts a leading `NOT` (`ASSUME x a`, `ASSUME NOT x a`); semantically it
is a *soft* assertion the engine is allowed to drop. This lets a small model
explore "what if?" — propose a guess, and let the engine do the backtracking.

During evaluation a soft fact behaves **exactly like a confident fact**: it seeds
the model, fires `RULE`s, and can satisfy or violate a `PREMISE`. The difference
shows only when the system is contradictory. Then the engine splits the blame:

```
hard = FACT/NOT + PREMISE + RULE          (commitments)
soft = ASSUME                             (hypotheses)
```

- If **hard alone is unsatisfiable**, the contradiction is in the commitments. The
  verdict is the usual `CONFLICT`; assumptions are not blamed (no retract set).
- If **hard is satisfiable but hard + soft is not**, the hypotheses are the cause.
  The engine computes the **minimal set of `ASSUME`s to retract** — an irreducible
  group that cannot all hold *together with every fact and premise* — by
  deletion-minimization over the soft constructs only (hard constructs stay
  pinned, so a `FACT`/`PREMISE` is **never** named). Dropping (or flipping) any one
  restores consistency.

The verdict stays **`CONFLICT`** (exit code 2) — a contradiction is a
contradiction — but the report carries a `retract` list instead of (and
superseding) the raw conflict pool, and names only hypotheses:

```
RESULT: CONFLICT
  RETRACT  your FACTs and PREMISEs are fine.
      But these ASSUME guesses cannot all be true together.
      Remove or flip ONE of them, then check again:
      ASSUME rel in_prod   [program.vrf:6]
      ASSUME NOT rel has_rollback   [program.vrf:7]
      ASSUME NOT rel has_feature_flag   [program.vrf:8]
EXIT_CODE: 2
```

In JSON this is the `retract` array; every item is tagged `"kind":"ASSUME"`, so a
caller can distinguish "drop a hypothesis" from "a commitment is wrong"
programmatically without a new status or exit code. Like a direct conflict, an
assumption clash that only emerges under case-splitting needs `BIDIRECTIONAL`;
clashes visible in the forward pass (the common case) are caught without it.

This is the engine's first **abductive** step: instead of only saying "no", it
points at exactly which hypothesis to revise — the foundation for later having the
engine *propose* the missing hypothesis itself.

## DSL: keywords

**A purely boolean system.** The core is 5 concepts (`FACT`, `NOT`, `PREMISE`,
`RULE`, `CHECK`), plus `ASSUME` for *soft* (retractable) hypotheses, plus a few
words for the body of constraints and rules, plus `IMPORT` for reuse.

| Word | Meaning | Kind |
|---|---|---|
| `FACT` | a TRUE assertion | premise (unchecked) |
| `NOT` | a FALSE assertion | premise (unchecked) |
| `ASSUME` | a soft, **retractable** assertion (`[NOT]` atom) — a hypothesis | premise (unchecked, soft) |
| `PREMISE` | a first principle — **checked** | constraint |
| `RULE` | an inference rule — **produces a fact** | rule, forward chaining |
| `WHEN` / `AND` / `THEN` | implication body (in `PREMISE` and `RULE`) | |
| `EXCLUSIVE` / `FORBIDS` / `ONEOF` / `ATLEAST` | list constraints (in `PREMISE`) | |
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
forms (`PREMISE`/`RULE`) — as a vertical block with indentation, each condition on
its own line.

```
// Reuse: pull facts/premises/rules from another source
IMPORT "physics.vrf"

// Facts (TRUE) and negations (FALSE) — boolean atoms, no numbers, one line
FACT <Subject> <predicate> [<object>]
NOT  <Subject> <predicate> [<object>]

// List premise: EXCLUSIVE / FORBIDS / ONEOF / ATLEAST
PREMISE <name>:
    EXCLUSIVE
        <Subject> <predicate> [<object>]
        <Subject> <predicate> [<object>]

// Implication premise — CHECKED (violation → CONFLICT)
PREMISE <name>:
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

The difference between `PREMISE` and `RULE` with an identical WHEN/THEN body:
`PREMISE` **checks** (no convergence → CONFLICT), `RULE` **produces** a new fact.

## IMPORT — reuse over a source-agnostic engine

The engine is **source-agnostic: it consumes strings.** A file is merely one way
to reuse a body of facts and premises. Resolution goes through a `Resolver`
abstraction (mirroring vsm-grammar's `SourceResolver` / `MemoryResolver` /
`FileResolver`): `IMPORT "physics.vrf"` asks the resolver for the string named
`physics.vrf`, however it is stored (file, in-memory map, network).

This matters for a small model. The model can only err at the premise level; a
vetted premise library removes even that — the model writes only `FACT` lines and
pulls the first principles from a curated, reviewed library. It also saves
context: the generated file is just facts.

### Semantics: flat merge into one shared atom universe

`IMPORT` performs a **flat merge** of all `FACT` / `NOT` / `PREMISE` / `RULE` from
the imported source into the current set. Crucially, **atoms unify across sources
by identity** — an imported premise about `Engine.X has fuel` constrains the fact
`Engine.X has fuel` declared in the main file. There is **no alias namespacing of
atoms** (that would break unification, which is the whole point of importing
premises). Atoms live in one global namespace keyed by `(subject, predicate, object?)`.

### Duplicate premises are idempotent — not a conflict

Importing the same library twice, or two libraries sharing a lemma, is
automatically harmless. The engine compiles everything to a **set** of
`Impossible([...])` clauses (CNF); feeding an identical clause to the solver is a
no-op, because `P ∧ P ≡ P`. No special handling is needed at the logic level.

Two bookkeeping guards exist on top, and content-addressing (sha256, mirroring
vsm-guard's CAS) is the natural tool for both:

- **Dedup for reports.** The same premise is not listed twice in a conflict pool;
  identical normalized content → identical content hash → one first principle.
- **Redefinition is an error only within one source.** Premise/rule names are
  per-source **labels**, not global identifiers — nothing references a premise by
  name across files, so two different files (domains) may reuse a name with
  different bodies; both apply, and the report qualifies them by source
  (`physics.vrf:safety` vs `biology.vrf:safety`). Reusing a name with a different
  body *inside the same source* is a genuine `PremiseRedefinition` error.

> This is the one place we deliberately diverge from vsm-grammar. vsm
> hash-namespaces rules so they stay *apart* and are referenced by alias. We need
> the opposite for **atoms** (they must unify across files — that is the value of
> importing premise libraries), so atoms are global. We apply the same idea vsm uses
> for namespacing only to the human-facing **labels** (per-source scoping) — never
> to atoms — and there is no `AS` alias because premises are never referenced by name.

### Cycle detection and dedup of sources

`IMPORT` cycles (`a` imports `b` imports `a`) and repeated imports are handled
exactly like vsm's `VstCompiler`: each resolved source is hashed (sha256), a
`visit_stack` of hashes detects circular dependencies, and an already-compiled
hash is reused rather than re-parsed.

> Note: the sha256 content-addressing is used **only** for source/premise dedup,
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

statement   = import | fact | negation | assume | premise | rule | check ;

import      = "IMPORT" , string , NEWLINE ;
fact        = "FACT" , atom , NEWLINE ;
negation    = "NOT"  , atom , NEWLINE ;
assume      = "ASSUME" , literal , NEWLINE ;   (* soft: literal allows a leading NOT *)
check       = "CHECK" , [ subject ] , [ "BIDIRECTIONAL" ] , NEWLINE ;

premise       = "PREMISE" , name , ":" , NEWLINE , ( list_body | impl_body ) ;
rule        = "RULE"  , name , ":" , NEWLINE , impl_body ;

list_body   = list_op , NEWLINE , atom_line , atom_line , { atom_line } ;  (* >= 2 *)
list_op     = "EXCLUSIVE" | "FORBIDS" | "ONEOF" | "ATLEAST" ;
atom_line   = atom , NEWLINE ;

impl_body   = when_line , { cont_line } , then_line , { cont_line } ;
when_line   = "WHEN" , literal , NEWLINE ;
then_line   = "THEN" , literal , NEWLINE ;
cont_line   = ( "AND" | "OR" ) , literal , NEWLINE ;
            (* one group (antecedent or consequent) may not mix AND and OR *)

atom        = subject , predicate , [ object ] ;
literal     = [ "NOT" ] , atom ;
subject     = identifier ;
predicate   = identifier ;
object      = identifier ;
name        = identifier ;
string      = '"' , { any-char-except-quote } , '"' ;

identifier  = letter , { letter | digit | "_" | "." } ;
letter      = ? any Unicode letter (Cyrillic, CJK, Latin, …) ? ;
digit       = ? any Unicode digit ? ;
```

Identifiers accept letters of **any** script, so `условие`, `名前` and `motor` are
all valid. The first character must be a letter (never a digit, `_`, `.`, or
punctuation); subsequent characters may also be digits, `_`, or `.`. Keywords stay
ASCII CAPS (see the reserved list below) — like SQL, only the keywords are fixed to
one language, the names are yours.

How the parser finds the end of an `PREMISE`/`RULE` block: the block continues while
lines start with body words (`WHEN`/`AND`/`THEN` or a `list_op`) or with an
identifier (list atoms), and ends at the first line with a top-level word
(`IMPORT`/`FACT`/`NOT`/`PREMISE`/`RULE`/`CHECK`) or at EOF. An `AND` before `THEN` is
an antecedent condition; an `AND` after `THEN` is an additional consequent.

Reserved words (always CAPS, in full): `IMPORT FACT NOT ASSUME PREMISE RULE CHECK
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

PREMISE one_path:
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
their numeric order is expressed by an **premise**, with no arithmetic at all:

```
PREMISE speed_order:
    WHEN Motor over_200
    THEN Motor over_100
// if speed > 200, then it is NECESSARILY > 100 — pure logic
```

So the engine catches the bug "you handled >200 as the slow path, even though >200
also means >100" — all without a single number.

## The engine's two passes

### Forward pass

```
facts + premises → saturate with rules → check constraints → result
```

Phases:
1. **Parse** — tokenize DSL → AST
2. **Bind** — build `FactStore: Map<Atom, (Value, Source)>`, where
   `Atom = (subject, predicate, object?)` in full (see the atom-identity invariant)
3. **Saturate** — apply `RULE` rules to a fixpoint; on each step check non-contradiction
4. **Verify** — for each `PREMISE`: evaluate(constraint, fact_store)
5. **Report** — collect results with provenance (where each fact came from)

### Backward pass (model finding)

Triggered by `CHECK X BIDIRECTIONAL`, or always on `CHECK X` in strict mode.

```
take the current fact set → search for an alternative fact set
that also satisfies all premises → if found → UNDERDETERMINED
```

Example:
```
FACT A has flying
PREMISE ax:
    EXCLUSIVE
        A has flying
        A has swimming

Forward:  CONSISTENT
Backward: A has swimming also satisfies all premises
→ UNDERDETERMINED: an alternative model exists
  hint: add  NOT A has swimming  to pin it down unambiguously
```

## The engine's four results

```
CONSISTENT       — no contradictions, a unique model
UNDERDETERMINED  — the logic does not break, but there is an alternative interpretation
CONFLICT         — a premise is violated, a contradiction was found
WARNING          — not enough data to check (UNKNOWN in a critical place)
```

### Output format

```
CHECK: <Subject>
  [CONSISTENT|UNDERDETERMINED|CONFLICT|WARNING]  <premise>  <details>
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

The engine shows what converges, and shows what cannot be computed — because an
UNKNOWN component takes part in the needed inference.

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
2. For each rule/premise compute the antecedent by three-valued AND (Kleene):
      - any input FALSE   → antecedent FALSE → the rule does NOT fire,
                            implication vacuously satisfied → CONSISTENT, NO warning
      - all inputs TRUE   → antecedent TRUE  → go to THEN (step 3)
      - otherwise (some UNKNOWN, no FALSE) → antecedent UNKNOWN → branch blocked (step 4)
3. Antecedent TRUE:
      RULE:  derive the THEN fact = TRUE (if it is already FALSE → CONFLICT)
      PREMISE: check THEN — TRUE→CONSISTENT, FALSE→CONFLICT, UNKNOWN→WARNING
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
| **CONFLICT pool** | which facts+premises together produce a contradiction | unsat-core from SAT (or computed ourselves) |
| **WARNING pool** | which branches are blocked and by which UNKNOWN component | our layer, during forward chaining |
| **CONSISTENT** | can be hidden; or list all successful combinations | our layer |

CONFLICT immediately shows its pool — who conflicts with whom. WARNING by analogy
shows its pool — where and because of what it got stuck. CONSISTENT may stay
silent or list successful combinations for clarity.

### Example WARNING pool

```
WARNING pool (2 blocked branches):
  [branch 1]  PREMISE wings_need_bone
              blocked by: has wing = UNKNOWN
              chain: FACT has flying → WHEN has flying THEN has wing AND has bone → STOP at has wing
  [branch 2]  PREMISE needs_fuel
              blocked by: has engine = UNKNOWN
              chain: RULE WHEN has engine THEN needs fuel → STOP at has engine

  hint: add  FACT ... has wing  and  FACT ... has engine
        to unblock both branches
```

So the model sees not just "not enough data," but **precisely where and in which
chain** it got stuck — and exactly what to add.

### Conflict explainability: the derivation trace and the unsat core

A bare `CONFLICT` is not actionable — "a premise is violated, go find why" puts the
work back on the reader. Two mechanisms answer *why*:

**1. Derivation trace (forward-pass conflicts).** When a premise fires because some
atoms were forced TRUE/FALSE, the forward pass already *knows* how each was forced:
either a `FACT`/`NOT`, or a `RULE` whose antecedent held. Each `Conflict` carries a
`trace` — the chain of those forcings, supports first, ending at the conflict:

```
CONFLICT  mortal_xor_immortal (EXCLUSIVE)  [socrates.vrf:29]
    socrates is mortal
    socrates is immortal
    why:
      socrates is human  = TRUE   [FACT socrates.vrf:13]
      socrates is animal = TRUE   from humans_are_animals (RULE) [..:17]  <= socrates is human
      socrates is living = TRUE   from animals_are_living (RULE) [..:21]  <= socrates is animal
      socrates is mortal = TRUE   from living_things_are_mortal (RULE) [..:25]  <= socrates is living
      socrates is immortal = TRUE [FACT socrates.vrf:14]
```

The chain is exact, deterministic, and needs no SAT machinery — it is read straight
off the provenance the forward pass recorded. A direct `FACT X` + `NOT X` and the
`<system>` unsatisfiability conflict have no chain, so their `trace` is empty.

**2. Minimal unsat core (backward-pass conflicts).** Some systems are jointly
unsatisfiable without any single premise firing in the forward model (the
contradiction only appears under case-splitting). The backward SAT pass detects
this; to name *who* is to blame we compute a **1-minimal unsat core** by
deletion-based minimization: drop each construct (fact / premise / rule) in turn and
re-solve — if the rest is still UNSAT the construct was not needed. What survives is
an irreducible set, reported as `unsat_core`:

```
CONFLICT  - (UNSAT)  [<system>:0]
    the premises and facts are jointly unsatisfiable
  CORE  smallest jointly-unsatisfiable set (4):
        one_ab (ONEOF)   [..:1]
        a_implies_c (PREMISE) [..:5]
        b_implies_c (PREMISE) [..:8]
        x c (NOT)        [..:11]
```

This costs O(n) SAT calls over the constructs — fine at our scale, and needs no
proof logging (which the in-crate SAT core deliberately omits). A premise that
desugared into several clauses is grouped back by origin, so the core blames whole
premises, not clause shards.

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

### Conflict among the premises themselves
Premises can be incompatible even without facts (e.g. they mutually require
exclusive things). SAT will catch this: the CONFLICT pool then names premises, not facts.

### Duplicate premises across imports
Identical constraints (same content hash) are idempotent — merged into one clause,
never a conflict (`P ∧ P ≡ P`). Premise/rule names are per-source labels: the same
name in two different files (domains) is fine (both apply, qualified by source);
the same name with a different body *within one source* is a `PremiseRedefinition`
error. See the IMPORT section.

### CHECK scope
`CHECK <Subject>` reports on premises and rules where this subject participates.
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
`THEN NOT X` means: for a `PREMISE` — X must be FALSE; for a `RULE` — derive X=FALSE.

---

# Part IV. Implementation

## What we take ready-made, what we write ourselves

We write no heavy math. Everything of our own is the parser, the gluing, and the formatting.

| Layer | How it is covered |
|---|---|
| `.vrf` parser | written by us (`nom` + `nom_locate`, `no_std`) — crate `elenchus-parser` |
| Import resolution, desugaring, atom interning, content-addressing | written by us — crate `elenchus-compiler` |
| Boolean logic, CONFLICT / CONSISTENT | a small CDCL SAT core (varisat's algorithm), `no_std` — crate `elenchus-solver` |
| Backward pass / UNDERDETERMINED | that same SAT core in all-SAT mode (enumerate all models) |
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
    elenchus-solver/    no_std; 3-valued forward pass + a small CDCL SAT core
```

Dependency versions are pinned 1:1 with the vsm workspace (`nom` 8, `nom_locate`
5, `thiserror` 2, `sha2` 0.10). The parser is our own (English-like, not
S-expressions), but in vsm-parser's style: zero-copy over `&str`, `Span`/`Located`
for line/column tracking, and a human-friendly `^--- here` error display.

> **Note on the SAT core.** A SAT solver knows only TRUE/FALSE — not UNKNOWN or
> WARNING, so the WARNING pool is carried in our own layer on top. For
> UNDERDETERMINED it enumerates models (find a solution, add a blocking clause,
> solve again — all-SAT). The one place to be careful: UNKNOWN must NOT be passed
> to SAT (collect it separately). Passing UNKNOWN as false fabricates CONFLICTs;
> as true fabricates CONSISTENTs.

## Scope: boolean only

The engine checks structural consistency: contradictions, cycles, missing
premises, underdetermination — what a small model tends to lose over a long chain.

It does **no** arithmetic or computation; those stay in the generated code and its
tests. For helping a model plan and write code, the boolean checks are enough.

## Integration (plan)

1. Standalone Rust CLI: `elenchus-cli file.vrf`
2. JSON output: `elenchus-cli file.vrf --format json`
3. An npm wrapper → a `planner_verify` tool in pi-code-planner
4. Insertion point in the state machine: an `execution/contract_check` step
   or a new `verify_reasoning` step between discovery and planning

elenchus is an independent project; integration into pi-code-planner is a later,
optional step via the npm wrapper.

---

# Part V. What this is not (and the alternatives)

This is a **boolean SAT checker with three-valued logic** (TRUE / FALSE /
UNKNOWN) on top, and nothing else. It does not do numbers/arithmetic,
quantifiers (∀/∃), probabilities, nested premises, or type/schema declarations.
Numbers become named boolean atoms (`Motor over_100`); their order is stated as an
premise — see "How code tasks map into boolean logic". That keeps the language
small enough for a local model to write reliably.

If you need more than boolean consistency, use a different tool — they already
exist, there's no reason to reinvent them:

- **Plain SAT** — if you just want a solver to embed/study, [varisat](https://github.com/jix/varisat)
  is the CDCL solver whose algorithm our `elenchus-solver` follows.
- **Arithmetic / numeric proofs (SMT)** — if you need to prove things *about
  numbers* (`x > 0 ⇒ x + 1 > 1` for all `x`), that is SMT, which is strictly more
  powerful. Use [z3](https://github.com/Z3Prover/z3) (Rust crate `z3-sys`); its
  input language is SMT-LIB.

For code, arithmetic is usually checked by the compiler and tests anyway, so the
boolean checker covers a large class of bugs without a single number. Structural
consistency (contradictions?) and numeric correctness (right value?) are
independent — this tool only does the first.

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
PREMISE fly_xor_swim:
    EXCLUSIVE
        Creature.A has flying
        Creature.A has swimming

PREMISE wings_need_bone:
    WHEN Creature.A has flying
    THEN Creature.A has wing
    AND  Creature.A has bone

PREMISE no_dual_temp:
    FORBIDS
        Creature.A has warm_blood
        Creature.A has cold_blood

// Inference rule — produces a new fact
RULE needs_oxygen:
    WHEN Creature.A has flying
    THEN Creature.A needs oxygen

CHECK Creature.A BIDIRECTIONAL
```
