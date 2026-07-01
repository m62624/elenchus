---
name: elenchus
description: >-
  Mechanically check that a set of facts and constraints is logically consistent
  — instead of trusting your own long chain of reasoning. Reach for it ANY time
  correctness depends on a web of interacting conditions that are easy to get
  subtly wrong, in logic OR in code: role/shift/seat assignments, "exactly one
  of", mutually-exclusive states or feature flags, readiness/deploy gates,
  if/else and state-machine branch coverage, access-control rules,
  dependency/ordering constraints, config invariants, permission matrices, or
  multi-step logic puzzles. You write facts and first principles in a tiny
  English-like DSL; the elenchus engine (a three-valued SAT checker) does the
  logic and answers CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT and shows
  why. Use it to catch contradictions and gaps a hand-derived argument would miss,
  and iterate until it answers CONSISTENT.
---

# elenchus — a logical-consistency checker

A model is good at stating facts but bad at holding a long logical chain without
quietly contradicting itself. elenchus moves the logic **out** of the model: you
state only **facts** and **first principles (premises)**; a Rust engine does the
inference and finds contradictions mathematically. You can only be wrong at the
premise level — and that is caught immediately.

**What it is:** a simplified SAT (boolean *satisfiability*) checker. Given a pile
of TRUE/FALSE constraints it finds an assignment satisfying all of them, or proves
none exists. elenchus adds a third truth value, **UNKNOWN**, and reports a verdict
instead of a raw sat/unsat. It does **not** do arithmetic or proofs about numbers
(that is SMT, a bigger tool) — only boolean structure.

> **This file is the whole language.** Every keyword and form is listed below.
> **If a construct is not described here, it does not exist** — there are no
> parentheses, no operators inside a name, no nesting, no arithmetic. When you
> need something the language lacks, model it as boolean atoms (see "What does
> not exist").

## The loop — this is the point

Running once is not the job. The verdict tells you what to do next; you iterate
until it is **CONSISTENT**.

```
   write facts + premises ─▶ run ─▶ CONSISTENT?  ── yes ─▶ done (exit 0)
            ▲                         │ no
            │                         ▼
            └──── add facts / fix or rethink a premise ◀── read the verdict
```

| Verdict | exit | Meaning | Your next move |
|--------|:----:|---------|----------------|
| **CONSISTENT** | 0 | no contradiction; answer pinned down | done |
| **WARNING** | 1 | a premise can't be checked — a needed atom is UNKNOWN | add the `FACT`/`NOT` it names under `blocked by:` — or, if that atom should follow automatically from an already-true `WHEN`, make it a `RULE` (which derives it) instead of a `PREMISE` |
| **UNDERDETERMINED** | 1 | satisfiable, but several models fit | add the fact it suggests (`pin it down: add …`) |
| **CONFLICT** | 2 | a premise is violated, or premises are jointly unsatisfiable | a fact is wrong, or two principles can't both hold — fix one |

**The target before you act on the reasoning is `CONSISTENT` (exit 0) — nothing
else.** WARNING, UNDERDETERMINED and CONFLICT are *not* "done"; each is the engine
telling you exactly what is missing or wrong (see the "Your next move" column).
Treat any non-zero verdict as "keep going", not as a result to report.

**But `CONSISTENT` is only as good as what you fed it.** It certifies "no
contradiction *among the facts and premises you wrote*" — it cannot vouch for a
constraint you never stated. A CONSISTENT verdict on an under-specified model is
false confidence. So:

- **Before the first run, account for everything:** encode every genuine first
  principle as a `PREMISE` (mutual exclusions, "exactly one of", required-together
  gates, orderings, branch coverage) and state every fact you actually know. A
  missing premise = the engine will happily say CONSISTENT about a broken model.
- **Reach CONSISTENT by stating more truth, never by gaming the check** — don't
  delete a valid premise or assert something you don't know just to turn it green;
  that throws away the very check you wanted. If you genuinely don't know a fact,
  that uncertainty *is* the finding.

So "done" = **CONSISTENT with every real premise present and every known fact
stated** — no remaining "but…". Only then trust the reasoning and proceed.

**"Keep going" means fix an incomplete model — not force the verdict green.**
Some questions *ask whether a consistent configuration exists at all*, and the
honest answer is that none does. A `CONFLICT` (or `UNDERDETERMINED`) is then the
**result, not a failure** — but *only* once you reached it by stating real truth
and its `why:`/`CORE` traces solely to genuine first principles and true facts
(examples 6 and 8 are exactly this). Before you accept any non-`CONSISTENT`
verdict as the answer, first rule out the model-level causes that would flip it:
a wrong `FACT`, a missing `PREMISE`, a typo'd atom (`HINT`/`ORPHAN`), or a
value-establishing `PREMISE` that should be a `RULE`. A verdict that survives all
of that is a **proof** — report it with its `CORE`/`why:` trace. The one move
never allowed is declaring `CONFLICT`/UNSAT *because you got stuck* or to stop
trying: UNSAT is a claim that demands the very same rigour `CONSISTENT` demands of
completeness. No `CORE`/`why:` to stand on, no answer — keep going.

## Keyword reference — the COMPLETE vocabulary

Keywords are **ALWAYS CAPS, ASCII**; everything else is your content. **This table
is the entire language — every keyword elenchus has.** If a word or form is not in
this table, **it does not exist**: no other keywords, no operators, no parentheses,
no nesting, no arithmetic. An unknown form is a hard error, not a hidden feature —
**never invent syntax.** When you need something absent, model it as boolean atoms
(see "Nothing else exists"). If the version check passed, you can trust this
completeness exactly.

| Keyword | Where | One-line meaning |
|---------|-------|------------------|
| `DOMAIN` | statement (first, once) | declare this file's domain — the namespace its atoms live in (**required**) |
| `FACT` | statement | assert an atom TRUE |
| `FACT … BECAUSE …` | statement | assert an atom TRUE **and name its ground**; the engine checks the ground holds (FALSE → CONFLICT, UNKNOWN → WARNING) |
| `NOT` | statement / literal prefix | assert an atom FALSE (or negate a literal in a body) |
| `ASSUME` | statement | a **soft, retractable** hypothesis (`[NOT] atom`) — acts like a fact, but on a clash the engine says which to drop |
| `PREMISE` | statement | a **checked** first principle (violation → CONFLICT) |
| `RULE` | statement | an implication that **derives** new facts (forward chaining) |
| `CHECK` | statement | run the engine (optionally for one subject) |
| `BIDIRECTIONAL` | `CHECK` modifier | also run the backward SAT pass (finds UNDERDETERMINED + joint-unsat) |
| `IMPORT` | statement | pull in another `.vrf` source; reference its atoms as `<domain>.<atom>` |
| `AS` | `IMPORT` modifier | give the imported domain a local alias (`IMPORT "x.vrf" AS y`) |
| `EXCLUSIVE` | `PREMISE` body | **at most one** of the listed atoms is TRUE |
| `FORBIDS` | `PREMISE` body | synonym of `EXCLUSIVE` (reads well for two: "not both") |
| `ONEOF` | `PREMISE` body | **exactly one** of the listed atoms is TRUE |
| `ATLEAST` | `PREMISE` body | **at least one** of the listed atoms is TRUE |
| `EXISTS … IN …` | `PREMISE` body | **at least one** element of a `SET` satisfies the condition (the ∃ dual of `FOR EACH`) |
| `EXISTS … WITNESS …` | `PREMISE` body | prove ∃ by **naming the one element** that satisfies it — no `SET`, grounds to a single atom (the open-domain ∃) |
| `WHEN` | `PREMISE`/`RULE` body | starts the antecedent of an implication |
| `THEN` | `PREMISE`/`RULE` body | starts the consequent |
| `AND` | `WHEN`/`THEN` group | conjunction of literals in that group |
| `OR` | `WHEN`/`THEN` group | disjunction of literals in that group |
| `SET` | statement | declare a finite set of elements to quantify over (one element per line) |
| `FOR EACH … IN …` | `PREMISE`/`RULE` header | instantiate the body once per element of a `SET`, binding a name |
| `FOR EACH … <rel> …` | `PREMISE`/`RULE` header | instantiate the body once per declared `FACT` pair of a relation |
| `CLOSE` | statement | `CLOSE <rel> TRANSITIVE\|SYMMETRIC\|REFLEXIVE\|EQUIVALENCE\|SCC` — close a relation at compile time (only `TRANSITIVE` rejects a cycle) |
| `VAR` | statement | declare an **external port** — a one-word proposition supplied from outside (`VAR <name> [DEFAULT true\|false]`) |
| `PROVIDE` | statement | bind a `VAR` port's value from data (`PROVIDE <name>: true\|false`) |
| `DEFAULT` | `VAR` modifier | the fallback value used when nothing is supplied for the port |
| `//` | anywhere | line comment (to end of line) |

The line-oriented rules that hold everywhere: **every file begins with `DOMAIN
<name>`** (its first statement); **one statement per line** (newlines separate
them); **indentation and extra spaces are cosmetic**; an **atom** is two or three
space-separated identifiers `subject predicate [object]`, optionally prefixed with
a domain (`<domain>.subject predicate [object]`); a **literal** is an atom
optionally prefixed with `NOT`.

## Each keyword — one card: **is · use when · form**

Every card's **form** is the *only* correct way to write that keyword; a form not
shown does not parse. **not:** lists what the engine rejects right there.

### `DOMAIN` — the file's namespace (required, first)
- **is** — the namespace every atom belongs to, part of each atom's identity
  (`physics.engine runs` ≠ `plan.engine runs`), so an import can't silently collide.
- **use when** — always: the first line of every file, exactly once.
- **form** — `DOMAIN <name>`  ·  **not:** omitted, repeated, or placed after any other statement.
```vrf
DOMAIN plan
FACT engine has_fuel          // identity: plan.engine has_fuel
```

### `FACT` / `NOT` — confident truth
- **is** — assert an atom TRUE (`FACT`) or FALSE (`NOT`). An atom you never state is
  **UNKNOWN** — the engine never guesses. The only way to assert truth.
- **use when** — you know a thing is (or is not) the case.
- **form** — `FACT <atom>` · `NOT <atom>`, where `<atom>` = `subject predicate [object]`
  ·  **not:** a bare one-word atom unless it is a declared `VAR`.
```vrf
FACT socrates is human
NOT  socrates is robot
```

### `FACT … BECAUSE …` — assert a fact **and name its ground**
- **is** — a `FACT` that names the ground it rests on; the engine checks that ground
  (TRUE → silent · FALSE → CONFLICT with a trace · UNKNOWN → WARNING). Evaluative: it
  adds no constraint, never forces the ground true (an unestablished ground is
  *reported*).
- **use when** — you want "how do you know?" checked. Opt-in; **chains compose** — a
  ground may itself be a `FACT … BECAUSE …`, and the weakest link surfaces (a bare
  asserted ground is accepted as a first principle).
- **form** — `FACT <atom> BECAUSE <ground-atom>`  ·  **not:** more than one ground; a
  ground on `NOT`/`ASSUME` (only `FACT` takes `BECAUSE`).
```vrf
FACT db reachable                        // the ground
FACT api healthy BECAUSE db reachable    // the claim, and the reason for it
```

### `ASSUME` — a soft, retractable hypothesis
- **is** — a *guess* that acts exactly like a `FACT`/`NOT` while checking (fires rules
  and premises), but on a clash the engine **keeps the facts/premises** and names which
  `ASSUME`s to drop (a `RETRACT` list) — it never blames a real fact.
- **use when** — exploring "what if?" without committing. **A `FACT` is never
  retracted; an `ASSUME` is.** On a `RETRACT`, drop or flip one listed `ASSUME`, re-check.
- **form** — `ASSUME <atom>` · `ASSUME NOT <atom>`  ·  **not:** used to state something
  you actually know (that is a `FACT`).
```vrf
FACT   rel reviewed
ASSUME rel in_prod            // what if this ships to prod?
ASSUME NOT rel has_rollback
```

### `PREMISE` — a checked first principle
- **is** — a constraint the engine **checks**; a violated premise is a CONFLICT. It
  only checks — it never establishes a value (that is a `RULE`).
- **use when** — a rule must hold: mutual exclusion, "exactly one", a required gate,
  an implication to verify.
- **form** — `PREMISE <name>:` then one body (a list body **or** a `WHEN…THEN` body) on
  the following lines; `<name>` is a report label.  ·  **not:** any body other than
  the ones below.
```vrf
PREMISE one_state:
    ONEOF
        door is open
        door is closed
```

### `EXCLUSIVE` / `FORBIDS` — at most one
- **is** — mutual exclusion: no two listed atoms are TRUE together (for n>2, pairwise
  "no two", not "not all at once"). `FORBIDS` is the same rule, reads well for a pair.
- **use when** — states/paths/flags that cannot co-occur.
- **form** — the keyword on its own line, then **≥2 atoms**, one per line (a `PREMISE`
  body).  ·  **not:** `NOT` on a list item; fewer than two atoms.
```vrf
PREMISE one_path:
    EXCLUSIVE
        motor uses fast_path
        motor uses slow_path
```

### `ONEOF` — exactly one (declares a variable)
- **is** — exactly one of the listed atoms is TRUE (at-most-one **and** at-least-one).
  Think "a variable whose value is one of the listed objects".
- **use when** — assignment ("each person has exactly one role"). **It also *closes*
  the variable:** after `ONEOF`, a value you never listed (a typo like `alice is leed`)
  is a **hard compile error** with `did you mean` — not a silent new UNKNOWN atom. The
  strongest typo guard the language has; closing is opt-in, per variable.
- **form** — keyword line, then **≥2 atoms** (a `PREMISE` body).  ·  **not:** `NOT` items.
```vrf
PREMISE alice_role:
    ONEOF
        alice is lead
        alice is dev
        alice is qa
```

### `ATLEAST` — at least one
- **is** — a disjunction with no upper bound: at least one listed atom is TRUE.
- **use when** — "at least one reviewer / backend / owner".
- **form** — keyword line, then ≥2 atoms (a `PREMISE` body).  ·  **not:** `NOT` items.
```vrf
PREMISE has_reviewer:
    ATLEAST
        pr reviewed_by_ann
        pr reviewed_by_bob
```

### `EXISTS … IN …` — at least one element of a SET
- **is** — "some element of a declared `SET` satisfies the condition" (∃ generated
  from a set, the dual of `FOR EACH … IN`). If every instance is forced false → CONFLICT
  (a coverage gap caught mechanically).
- **use when** — coverage over a listed set ("*some* handler takes it").
- **form** — `EXISTS <binder> IN <set>` then **one** condition line using the binder (a
  `PREMISE` body).  ·  **not:** more than one condition line.
```vrf
SET handlers
    auth
    billing
PREMISE someone_handles:
    EXISTS h IN handlers
        h handles request
```

### `EXISTS … WITNESS …` — prove ∃ by naming the one element
- **is** — ∃ over an **open** domain: instead of a `SET`, name the single element that
  works; the engine checks it holds (`EXISTS` over the singleton `{term}` — one atom;
  forced false → CONFLICT).
- **use when** — you can't list the whole domain but can point at one witness. (A
  **universal** must always name a `SET`/relation; only ∃ may point at a lone witness —
  that is what keeps it from ever blowing up.)
- **form** — `EXISTS <binder> WITNESS <term>` then **one** condition line (a `PREMISE`
  body).  ·  **not:** omitting both `IN` and `WITNESS` — an unwitnessed ∃ can't be
  checked → **WARNING** ("name a witness"), not a conflict.
```vrf
FACT auth_service handles request
PREMISE request_is_covered:
    EXISTS h WITNESS auth_service    // "some handler covers it — namely this one"
        h handles request
```

### `WHEN … THEN …` — implication (`AND` / `OR` group logic)
- **is** — "if the antecedent holds, the consequent must hold". As a `PREMISE`, a
  violation is a CONFLICT. Four shapes: `a∧b→c` · `(a∨b)→c` · `a→(c∨d)` · `(a∨b)→(c∨d)`.
- **use when** — conditional requirements and gates.
- **form** — `WHEN <lit>`, then zero+ `AND <lit>` **or** `OR <lit>`, then `THEN <lit>`,
  then zero+ `AND`/`OR`. Each `<lit>` is `[NOT] atom`.  ·  **not:** mixing `AND` and
  `OR` in one group (split into two premises); `AND`/`OR` or parentheses *inside* a literal.
```vrf
PREMISE deploy_gate:
    WHEN svc built
    AND  svc tested
    THEN svc deployable

PREMISE needs_a_backend:
    WHEN gateway in_prod
    THEN auth in_staging
    OR   api in_staging
```

### `RULE` — derive a new fact (forward chaining)
- **is** — an implication that *asserts* its `THEN` as a new fact when its `WHEN`
  holds — unlike `PREMISE`, which only checks.
- **use when** — a truth should follow automatically, **including ruling a branch out**
  with `THEN NOT y`. A `PREMISE WHEN x THEN NOT y` cannot establish `y`: while `y` is
  UNKNOWN it just stays blocked (WARNING). To *close* a branch (and reach CONSISTENT,
  not WARNING), **derive** the negation with a `RULE`.
- **form** — `RULE <name>:` then one `WHEN…THEN` body; `THEN` may be `NOT`.  ·  **not:**
  `OR` in `THEN` (a rule can't derive a disjunction — use a `PREMISE`); a list body.
```vrf
RULE not_repro_blocks_proof:    // if it can't be reproduced, it isn't proven
    WHEN NOT case reproducible
    THEN NOT case proven        // derived FALSE — the `proven` branch is closed
```

### `SET` + `FOR EACH … IN …` — write a premise once, apply per element
- **is** — `SET` declares a finite list; `FOR EACH <binder> IN <set>` on a header
  instantiates the whole body once per element, substituting the binder. "For all".
- **use when** — "every X must …" without hand-copying a premise per item.
- **form** — `SET <name>` then one element per line; then
  `PREMISE <name> FOR EACH <binder> IN <set>:` + body.  ·  **not:** a second `FOR EACH`
  on one header — nesting does not parse (keeps grounding linear, never element×element).
```vrf
SET tasks
    deploy
    backup
PREMISE one_slot FOR EACH t IN tasks:
    ONEOF
        t slot morning
        t slot night
```

### `FOR EACH <a> <relation> <b>` — quantify over declared pairs
- **is** — instantiate a body once per matching `FACT <a> <relation> <b>`, binding
  `a`→subject, `b`→object. The pairs are just facts you write.
- **use when** — relating **two** things (graphs, adjacency, dependencies) — the pair
  comes from data, so you never write a second binder.
- **form** — `PREMISE <name> FOR EACH <a> <relation> <b>:` + body; pairs from
  `FACT a rel b`.  ·  **not:** two free binders / joining two relations.
```vrf
FACT n1 linked n2
FACT n2 linked n3
PREMISE diff FOR EACH x linked y:    // neighbours can't share a colour
    FORBIDS
        x is red
        y is red
```

### `CLOSE <relation> <kind>` — close a relation at compile time
- **is** — a graph closure over the relation's `FACT` pairs at **compile time** (no
  solver cost); a relation `FOR EACH` then ranges over the closed set. `CLOSE`
  *replaces* the pairs.
- **use when** — reachability or grouping over declared edges.
- **form** — `CLOSE <relation> <kind>`, `<kind>` ∈
  `TRANSITIVE | SYMMETRIC | REFLEXIVE | EQUIVALENCE | SCC`.  ·  **not:** a cycle under
  `TRANSITIVE` (it doubles as a DAG check → error); the others allow cycles.
  - `TRANSITIVE` `a→c` when `a→b,b→c` · `SYMMETRIC` `b→a` · `REFLEXIVE` `x→x` ·
    `EQUIVALENCE` groups into classes · `SCC` isolates dependency cycles.
```vrf
FACT web depends_on api
FACT api depends_on db
CLOSE depends_on TRANSITIVE           // web depends_on db, transitively
```

### `CHECK` / `BIDIRECTIONAL` — run it
- **is** — runs the engine. Bare `CHECK` checks everything; `CHECK <subject>` restricts
  the report. `BIDIRECTIONAL` adds the backward SAT pass — reports **UNDERDETERMINED**
  (more than one model fits) and catches joint-unsat **CONFLICT**s that no single
  premise visibly violates (prints a `CORE`).
- **use when** — every program ends with a `CHECK`; add `BIDIRECTIONAL` when you care
  whether the answer is *unique*.
- **form** — `CHECK` · `CHECK <subject>` · `CHECK [<subject>] BIDIRECTIONAL`.
```vrf
CHECK alice BIDIRECTIONAL
```

### `IMPORT` / `AS` — reuse another domain
- **is** — pulls in another `.vrf` (which declares its own `DOMAIN`); you reference its
  atoms **explicitly** as `<domain>.<atom>` — no silent merge. To make an imported
  premise constrain a fact, write the fact *into* that domain (`FACT physics.Motor
  over_200`). `AS <alias>` renames the local handle (and disambiguates two imports of
  the same domain).
- **use when** — sharing a library of premises across files.
- **form** — `IMPORT "<path>"` · `IMPORT "<path>" AS <alias>` (quoted).  ·  **not:**
  resolves **only in file mode** (not `--text`/stdin); references are file-local and
  non-transitive (only domains *you* import here).
```vrf
DOMAIN demo
IMPORT "physics.vrf"
FACT physics.Motor over_200   // a fact placed into the imported domain
```

### `VAR` / `PROVIDE` / `DEFAULT` — external ports (templating)
- **is** — a `VAR <name>` is a one-word proposition whose truth is **supplied from
  outside**, so a `.vrf` becomes a template; a resolved port behaves exactly like
  `FACT <name>`/`NOT <name>`. `PROVIDE` binds a value from data; `DEFAULT` is the fallback.
- **use when** — one fixed logic re-run per task with different boolean inputs (a deploy
  gate per release). Mix ports freely with `FACT`/`PREMISE`/`RULE`; use `<name>` bare in
  any body.
- **form** — `VAR <name> [DEFAULT true|false]` · `PROVIDE [<domain>.]<port|atom>: true|false`.
  ·  **not:** a bare one-word atom with no `VAR` (undeclared → error).
- **resolution** — supplied **>** `DEFAULT` **>** UNKNOWN. Supply via CLI
  `--set "<name>:true"` / `--data <file>`, MCP `values`/`data`, or inline `PROVIDE`. A
  **multi-word** key (`engine has_fuel`) injects an **atom** like `FACT`. The
  PLACEHOLDERS section reports each port's status. Qualify a clashing name across imports
  as `domain.name`.
```vrf
DOMAIN deploy
VAR tests_green
VAR db_migrated DEFAULT false
PREMISE gate:
    WHEN tests_green
    AND  db_migrated
    THEN ship a
NOT ship a            // deny unless the gate forces it
CHECK
// CLI: elenchus deploy.vrf --set "tests_green:true db_migrated:true"  → CONFLICT (gate fires)
```

### `//` — comment
- **is** — a line comment, to end of line (own line or trailing). The only comment form.
- **form** — `// …`
```vrf
FACT a b   // a trailing note
```

## Atoms & identity — the #1 gotcha

An **atom**'s identity is its **domain** plus the triple
`(subject, predicate, object?)`, compared **verbatim, case-sensitively**. Bare
atoms take the file's `DOMAIN`; a `<domain>.` prefix puts the atom in an imported
domain. Identifiers may use letters of **any script** (`условие`, `名前` are fine),
then letters/digits/`_`; they can't start with a digit or equal a keyword. **`.` is
the domain separator, not a name character** — write compound names with `_`
(`Engine_X`, not `Engine.X`).

`has_fuel` ≠ `hasFuel` ≠ `Has_fuel`, and crucially **`is rolled_back` (two words)
≠ `is_rolled_back` (one word)** — `_`-vs-space changes which token is predicate vs
object, so they are *different atoms*. A typo silently makes a new UNKNOWN atom, so
the constraint you thought you wrote never fires. **Before each run, pick one
spelling per concept and make every name that should be one atom byte-identical.**
For any variable whose values are a fixed set, declare them with `ONEOF` — that
**closes** the variable, turning a misspelled value from a silent UNKNOWN atom into
a hard compile error with a `did you mean` (see `ONEOF`). It is the strongest typo
guard the language has; prefer it over relying on the advisory `HINT` below.

The engine helps with two advisory signals (neither **ever changes the verdict or
exit code**): a **`HINT`** (JSON: `hints`) when two names look like the same atom
typed two ways, and an **`ORPHAN`** (JSON: `orphans`) when a `FACT`/`NOT`/`ASSUME`
is referenced by no premise or rule — the usual sign a typo'd name failed to link
up, so the constraint you meant never fires. Both are nudges: fix the spelling if a
name should be one atom, wire up or delete a line that should not dangle.

## Nothing else exists — model it as booleans

**The vocabulary is closed.** A construct not listed above is not in the language:
the parser rejects it (a hard error, exit 2 — never a hidden feature). Once the
version check passes you can rely on this completely — **do not invent syntax** to
reach for something; encode it with the atoms and keywords you have. There is **no**:

- **numbers or arithmetic** (`>=`, `+`, "at most 3") — **none at all**. Turn a
  threshold into a named atom (`speed >= 100` → the atom `motor over_100`) and order
  them with a premise (`WHEN motor over_200 THEN motor over_100`). You have "exactly /
  at least / at most one" (`ONEOF` / `ATLEAST` / `EXCLUSIVE`); counts beyond one do not.
- **operators, `AND`/`OR`, or parentheses *inside* an atom or literal** — group logic
  across `WHEN`/`THEN` lines instead.
- **mixing `AND` and `OR` in one group**, or **`OR` in a `RULE`'s `THEN`**.
- **nested or unbounded quantifiers** — **one** `FOR EACH` per header; a **universal
  must name its domain** (`SET`/relation), only ∃ may leave it unnamed via one
  `WITNESS`. No join of two relations, no two free binders (route through one declared
  relation) — that keeps grounding linear, by construction.
- **probabilities, else/default branches, or `NOT` inside a list body.**

## Reading the report

The verdict is never a dead end — the report tells you *where* and *why*. Learn to
read these things (shown as the engine actually prints them).

**`blocked by:` + `fix:`** — on a `WARNING`, the UNKNOWN atom that stopped a premise
from being checked, plus a directed fix. The `fix:` line disambiguates the two
reasons a check stays blocked, so you don't guess:
```
  WARNING   needs_c (PREMISE)  [plan.vrf:5]
      blocked by: plan.c ready
      fix: nothing determines `plan.c ready` — add `FACT plan.c ready` (or `NOT …`), or if a PREMISE's THEN is meant to establish it, make that PREMISE a RULE so it derives the value
```
- *"nothing determines X"* → X is a free input: either state it (`FACT`/`NOT`), or,
  if a premise's `THEN` was meant to **establish** X, that premise should be a
  `RULE` (which derives) not a `PREMISE` (which only checks). This is the single
  most common small-model mistake — heed it.
- *"X is derived by a RULE that has not fired"* → a `RULE` *can* produce X; assert
  that rule's antecedent and X will be derived.

**`why:` trace** — on a violated premise, the derivation chain that forced the
clashing atoms (supporting facts first, then each rule built on them). Read it
top-down to the exact wrong step:
```
  CONFLICT  mortal_xor_immortal (EXCLUSIVE)  [socrates.vrf:29]
      socrates is mortal
      socrates is immortal
      why:
        socrates is human  = TRUE   [FACT socrates.vrf:13]
        socrates is animal = TRUE   from humans_are_animals (RULE) [..]  <= socrates is human
        socrates is living = TRUE   from animals_are_living (RULE) [..]  <= socrates is animal
        socrates is mortal = TRUE   from living_things_are_mortal (RULE) [..]  <= socrates is living
        socrates is immortal = TRUE [FACT socrates.vrf:14]
```
Here `socrates is immortal` (a fact) collides with `socrates is mortal`, which the
chain derived from `socrates is human`. Fix one link.

**`CORE`** — on a jointly-unsatisfiable system (found by `BIDIRECTIONAL`, where no
single premise is visibly violated), the smallest set jointly to blame:
```
  CONFLICT  - (UNSAT)  [<system>:0]
      the premises and facts are jointly unsatisfiable
  CORE  smallest jointly-unsatisfiable set (4):
        a_to_b (PREMISE) [..]      a_to_not_b (PREMISE) [..]
        need_a_or_c (ATLEAST) [..] c_to_a (PREMISE) [..]
```
Revisit exactly those four principles — one of them is wrong.

**`RETRACT`** — when your `FACT`s and `PREMISE`s are consistent but your `ASSUME`
guesses can't all be true at once, the engine names the smallest set of
hypotheses to drop. The verdict is still `CONFLICT` (exit 2), but the fix is
"drop or flip one guess", not "a fact is wrong":
```
  RETRACT  your FACTs and PREMISEs are fine.
      But these ASSUME guesses cannot all be true together.
      Remove or flip ONE of them, then check again:
      ASSUME rel in_prod   [program.vrf:6]
      ASSUME NOT rel has_rollback   [program.vrf:7]
      ASSUME NOT rel has_feature_flag   [program.vrf:8]
```
Drop (or flip) any one of those `ASSUME` lines and re-check. A `FACT`/`PREMISE`
is never listed here — only your hypotheses (JSON: `retract`, each item tagged
`"kind":"ASSUME"`).

**`HINT`** — advisory possible-typo nudge; **never changes the verdict**:
```
  HINT      possible typo — 'auth is rolled_back' and 'auth is_rolled_back' look like the same atom (...)
```

**`ORPHAN`** — advisory: a `FACT`/`NOT`/`ASSUME` whose atom is referenced by **no**
`PREMISE` and **no** `RULE`. It is logically inert — nothing checks it and nothing
is derived from it, so it can never produce a CONFLICT, WARNING or DERIVED.
**Never changes the verdict or exit code** (JSON: `orphans`). Almost always a
typo'd atom name (so the constraint you meant never links up) or a leftover line:
```
  ORPHAN    FACT lonley sits idle — not used by any premise or rule (no effect on the result)
```
Fix the spelling so it joins the atom you meant, wire it into a premise/rule, or
delete the line.

**`UNUSED IMPORT`** — advisory: a file `IMPORT`s a domain it never references (no
`domain.atom` from that file uses it), so the import is inert. **Never changes the
verdict or exit code** (JSON: `unused_imports`). Usually a leftover import or a
forgotten `domain.` prefix:
```
  UNUSED IMPORT  physics — imported in plan.vrf:2 but never referenced (no effect on the result)
```
Reference it (`FACT physics.…`) or drop the `IMPORT` line.

**JSON** (`--format json` / MCP) carries the same data for programmatic reading.
Always branch on `status`; the rest mirrors the human report:
```json
{ "status":"CONFLICT", "exit_code":2,
  "conflicts":[ { "premise":"m", "kind":"RULE", "source":"...", "line":2,
                  "atoms":["s mortal (derived value contradicts a known fact)"],
                  "trace":[ {"atom":"s human","value":true,"how":"asserted","kind":"FACT","from":[]},
                            {"atom":"s mortal","value":false,"how":"asserted","kind":"NOT","from":[]} ] } ],
  "warnings":[], "derived":[], "underdetermined":null, "unsat_core":[],
  "retract":[], "hints":[], "orphans":[], "unused_imports":[] }
```

**exit code** = the verdict (0 = CONSISTENT, 1 = WARNING/UNDERDETERMINED, 2 =
CONFLICT) — a ready CI gate.

## Worked examples — easy → hard

### 1. The smallest contradiction
```vrf
DOMAIN demo
FACT x a
NOT  x a
CHECK x
```
`CONFLICT` (exit 2): the same atom can't be TRUE and FALSE.

### 2. Exactly-one assignment
```vrf
DOMAIN roles
PREMISE alice_role:
    ONEOF
        alice is lead
        alice is dev
        alice is qa
FACT alice is lead
CHECK alice
```
`CONSISTENT`. Assert a second role (`FACT alice is dev`) → `CONFLICT`.

### 3. A gate — iterate WARNING → CONSISTENT
```vrf
DOMAIN svc
FACT svc built
PREMISE ready:
    WHEN svc built
    AND  svc tested
    THEN svc deployable
CHECK svc
```
`WARNING` (`blocked by: svc tested`). Add `FACT svc tested` and `FACT svc
deployable` → `CONSISTENT`. Add `FACT svc tested` but `NOT svc deployable` →
`CONFLICT` (the premise fires and is violated).

### 4. Disjunction — `OR` in `THEN`
```vrf
DOMAIN net
PREMISE needs_backend:
    WHEN gateway in_prod
    THEN auth in_staging
    OR   api in_staging
FACT gateway in_prod
NOT  auth in_staging
NOT  api in_staging
CHECK
```
`CONFLICT` — the gate fires but every backend is out of staging. Set any one to
`FACT … in_staging` → `CONSISTENT`. (`WHEN a OR b THEN c` mirrors this on the left:
it fires when *either* trigger holds.)

### 5. Derivation + branch coverage (`RULE` + `EXCLUSIVE`)
```vrf
DOMAIN motor
FACT motor over_100
RULE pick_fast:
    WHEN motor over_100
    THEN motor uses fast_path
RULE pick_slow:
    WHEN NOT motor over_100
    THEN motor uses slow_path
PREMISE one_path:
    EXCLUSIVE
        motor uses fast_path
        motor uses slow_path
CHECK motor
```
`CONSISTENT` (derives `uses fast_path`). If your logic could ever take both paths,
`one_path` reports `CONFLICT` — a branch bug caught structurally.

### 6. Jointly-unsatisfiable — only `BIDIRECTIONAL` finds it
```vrf
DOMAIN demo
PREMISE a_to_b:
    WHEN x a
    THEN x b
PREMISE a_to_not_b:
    WHEN x a
    THEN NOT x b
PREMISE need_a_or_c:
    ATLEAST
        x a
        x c
PREMISE c_to_a:
    WHEN x c
    THEN x a
CHECK x BIDIRECTIONAL
```
No single clause is visibly violated (all UNKNOWN), but the premises can't all hold
(c→a→both b and ¬b). elenchus reports `CONFLICT` and prints a `CORE` naming the
four premises jointly to blame.

### 7. Reuse via `IMPORT`
```vrf
// physics.vrf (DOMAIN physics) provides:
//   PREMISE fast_xor_slow (EXCLUSIVE Motor uses fast_path / slow_path)
DOMAIN demo
IMPORT "physics.vrf"
FACT physics.Motor uses fast_path
FACT physics.Motor uses slow_path
CHECK
```
`CONFLICT` — your facts, placed **into the physics domain** (`physics.Motor …`),
violate the imported premise about those same atoms. (`IMPORT` needs file mode.)

### 8. Capstone — a real "ship to prod?" decision (the payoff)

Everything together on one subject (`rel`, so every atom stays byte-identical):
`ONEOF` for the stage, an `AND` gate, an `OR` safety requirement, a `RULE` that
derives an obligation, and a chained gate. You believe the release is ready and
state what you know:

```vrf
DOMAIN ship
PREMISE one_stage:                 // a release is in exactly one stage
    ONEOF
        rel in_dev
        rel in_staging
        rel in_prod
PREMISE prod_needs_deployable:     // prod requires passing the gate
    WHEN rel in_prod
    THEN rel deployable
PREMISE deploy_gate:               // the gate: every check must pass
    WHEN rel code_reviewed
    AND  rel tests_green
    AND  rel security_scanned
    THEN rel deployable
RULE migration_needs_backup:       // a schema migration *implies* a backup is owed
    WHEN rel has_migration
    THEN rel needs_backup
PREMISE backup_gate:               // an owed backup must be verified
    WHEN rel needs_backup
    THEN rel backup_verified
PREMISE prod_needs_safety:         // in prod, need a rollback path OR a feature flag
    WHEN rel in_prod
    THEN rel has_rollback
    OR   rel has_feature_flag
// --- what you actually know ---
FACT rel in_prod
FACT rel code_reviewed
FACT rel tests_green
FACT rel security_scanned
FACT rel deployable
FACT rel has_migration
FACT rel has_feature_flag
CHECK rel
```

`WARNING` — `blocked by: rel backup_verified`. You thought you were ready, but the
engine found the gap you didn't account for: `has_migration` makes the `RULE`
derive `needs_backup`, and `backup_gate` then requires `backup_verified`, which you
never stated. **This is the payoff** — a missed obligation surfaced mechanically,
not by luck.

Now make it honest, not green-at-any-cost:
- If the backup really was verified → add `FACT rel backup_verified` → `CONSISTENT`.
- If it was **not** → add `NOT rel backup_verified`: now it's a `CONFLICT` whose
  `why:` prints the chain `has_migration → needs_backup → backup_verified = FALSE`.
  The correct action is **don't ship**, not delete the premise.

(Drop `FACT rel has_feature_flag` and `prod_needs_safety` surfaces too: in prod
with neither a rollback nor a flag stated → `WARNING` blocked by both, telling you
to confirm a safety mechanism.)

### 9. Explore a hypothesis without committing — `ASSUME`

Same safety gate, but now you're *trying something on*: "what if this ships to
prod with no rollback and no feature flag — does it hold?" State the real
facts/premises, then guess with `ASSUME`:
```vrf
DOMAIN ship
FACT  rel reviewed
PREMISE prod_needs_safety:        // in prod: need a rollback OR a feature flag
    WHEN rel in_prod
    THEN rel has_rollback
    OR   rel has_feature_flag
ASSUME rel in_prod
ASSUME NOT rel has_rollback
ASSUME NOT rel has_feature_flag
CHECK rel
```
`CONFLICT` with a `RETRACT` list of all three guesses: with no rollback and no
flag, `rel in_prod` cannot satisfy `prod_needs_safety`. Your `FACT` and the
premise were never in question — only the hypotheses. Drop or flip any one (e.g.
`ASSUME rel has_feature_flag`) → `CONSISTENT`. That is the engine doing the
backtracking for you: it tells you exactly which guess to revise.

### 10. Encode a constraint / NP problem (the recipe)

elenchus *is* a SAT checker, so problems shaped like "assign values to variables
subject to constraints" (graph colouring, scheduling, seating, Sudoku-style grids
— the classic NP-complete family) map onto it directly. With `SET` + `FOR EACH`
you state each rule **once** instead of copying it per element. The recipe:

1. **The things → a `SET`**; **each one's value → a `ONEOF`** under
   `FOR EACH … IN <set>` (states "exactly one value" *and* closes the variable, so
   a mistyped value is a hard error, not a silent bug).
2. **Relations (edges, "depends on") → plain `FACT`s**; **each constraint over a
   pair → a `PREMISE FOR EACH x <rel> y`**. To follow a chain transitively, add
   `CLOSE <rel> TRANSITIVE` (which also rejects cycles).
3. **`CHECK BIDIRECTIONAL`** — `CONFLICT` = no valid assignment exists;
   `UNDERDETERMINED` = solvable but more than one assignment fits (not unique);
   `CONSISTENT` = exactly one.

Graph 3-colouring of a triangle — three nodes, every pair adjacent (so it needs 3
colours and the colouring is not unique). Note how the colour rule is written
once with `FOR EACH`, and the edges are just facts:

```vrf
DOMAIN graph
SET nodes
    n1
    n2
    n3
PREMISE colour FOR EACH n IN nodes:      // each node takes exactly one colour
    ONEOF
        n is red
        n is green
        n is blue
FACT n1 linked n2                        // the edges — plain facts
FACT n2 linked n3
FACT n1 linked n3
PREMISE diff_red FOR EACH x linked y:    // adjacent nodes can't share a colour
    FORBIDS
        x is red
        y is red
PREMISE diff_green FOR EACH x linked y:
    FORBIDS
        x is green
        y is green
PREMISE diff_blue FOR EACH x linked y:
    FORBIDS
        x is blue
        y is blue
CHECK BIDIRECTIONAL
```

Drop a colour from the `ONEOF` (2-colour a triangle) and the verdict flips to
`CONFLICT` with a `CORE` naming the clashing constraints. **Note the scale limit:**
the backward (`BIDIRECTIONAL`) pass explores assignments, so keep element/value
counts modest — elenchus is for *checking the logic of* a constraint problem, not
an industrial-scale solver.

## Run it — do these three steps first, in order (every session)

Before you write a single program, set up and verify the engine. Do **not** skip
ahead and start checking logic — an unconfirmed or mismatched engine makes every
later result untrustworthy.

### Step 0a — pick your transport (CLI **or** MCP)

You have exactly one of two ways in. Detect which:

- **Shell available →** use the **CLI** (the `elenchus-cli` binary).
  - `elenchus-cli program.vrf` — check a file (the only mode where `IMPORT` resolves).
  - `elenchus-cli --text "DOMAIN d\nFACT x a\nCHECK x"` — inline (newlines required between lines).
  - `cat program.vrf | elenchus-cli` — stdin. `--format json` for machine output.
  - **Templates (`VAR` ports):** supply values with `--set "<name>:true <name2>:false"`
    (space-separated, repeatable) and/or `--data values.vrf` (a file of `PROVIDE` lines).
    Qualify a clashing name across imports as `--set "domain.name:true"`. `--hide-params`
    drops the PLACEHOLDERS section from the human report.
- **No shell, but your tools include `elenchus_check` →** use **MCP**. Call
  `elenchus_check` with `{ "program": "<.vrf text>", "format": "json" }`
  (`\n`-separated lines). The entry is **either** inline `program` **or** a
  filesystem `path` (not both). With `program`, send any `IMPORT`ed sources inline
  as `"files": { "physics.vrf": "<.vrf text>", … }` (works on any server, no
  filesystem needed); use `"path": "<file.vrf>"` only if the server runs locally
  and you want it to read from disk like the CLI. Bind `VAR` ports with
  `"values": { "<name>": true }` and/or `"data": { "vals.vrf": "PROVIDE <name>:
  true\n" }`; a key may be qualified (`"domain.name"`) or name a multi-word atom
  (`"engine has_fuel"`). On a local server, `"data_paths": ["vals.vrf"]` reads
  PROVIDE files off disk (the npm wrapper's `dataFiles` is the Node equivalent).
  The server also has `elenchus_version` and `elenchus_about`.
- **On a syntax error** (either transport) you get the errors **grouped by
  class** (one per keyword) instead of a verdict — the correct syntax and an
  example shown once per class, with every offending place (line, caret, the
  problem) listed beneath. Every error is shown in one pass; fix the listed lines
  and re-run. By default all are shown; cap a flood with `--max-classes N` /
  `--max-per-class N` (CLI) or the `max_classes` / `max_per_class` arguments
  (MCP).
- **Neither →** elenchus is **not installed here**. Stop and send the user to
  **https://github.com/m62624/elenchus** to install it (how depends on their
  CLI/harness). Do **not** fabricate verdicts — if you can't run it, say so.

### Step 0b — smoke-test the transport you picked

Prove at least one transport actually runs before trusting anything. Run the
known-CONFLICT program and confirm the result:

- **CLI:** `elenchus-cli --text "DOMAIN d\nFACT x a\nNOT x a\nCHECK x"` → expect **CONFLICT**, exit **2**.
- **MCP:** `elenchus_check` with `{"program":"DOMAIN d\nFACT x a\nNOT x a\nCHECK x"}` → `status` == `"CONFLICT"`.

Any other answer means the install is broken — fix that before continuing.

### Step 0c — version check (MANDATORY; on ANY mismatch, STOP)

This skill targets the version in the marker below. Read the engine's version and
**print one explicit line** comparing them — never just note it silently:

```
elenchus version check: skill <marker> vs engine <reported> → OK | MISMATCH
```

<!-- skill-version: 0.13.0 -->

- **CLI:** `elenchus-cli --version` (or `-V`) → `elenchus-cli x.y.z`.
- **MCP:** call `elenchus_version` → `elenchus x.y.z` (you can't see
  `initialize`'s `serverInfo.version`, so use this tool).

**If engine == marker** → print `→ OK` and proceed.

**If they differ in ANY way — it does not matter which side is newer or older —
then the functionality the skill describes and the functionality the binary
provides have diverged, so any result may be wrong. Do ALL of this:**

1. **STOP.** Do not run the user's actual task. Abort the current execution.
2. **Immediately emit this warning to the user:** *"⚠️ elenchus version mismatch:
   this skill targets `X` but the installed engine reports `Y`. They describe
   different functionality — one of them must be updated to match the other
   (update the elenchus binary, or update this skill) for compatibility. Update
   from https://github.com/m62624/elenchus."*
3. This warning is **advisory for the user** — but for **you (the model) it is a
   hard stop**: do **not** rationalize it as "probably fine". **Ask the user to
   explicitly confirm** whether to proceed anyway, and make clear that **any
   results produced under a version mismatch may be unreliable / not at the
   correct level**, because the engine's behaviour may not match what this skill
   assumes.
4. Proceed **only if the user explicitly says to continue**. If they do, tag every
   result *"unverified — version mismatch"* until the versions are aligned.
