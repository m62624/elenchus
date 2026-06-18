---
name: elenchus
description: >-
  Check that a set of facts and constraints is logically consistent — catch
  contradictions, missing data, and under-specified problems. Reach for it
  whenever correctness depends on a web of conditions that are easy to get
  subtly wrong: role/shift/seat assignments, "exactly one of these",
  mutually-exclusive states, readiness/deploy gates, if/else branch coverage,
  state-machine legality, or invariants across many flags. You write facts and
  first principles in a tiny English-like DSL; the elenchus engine does all the
  logic and answers CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT. Keep
  refining and re-running until it answers CONSISTENT.
---

# elenchus — a logical-consistency checker

A model is good at stating facts but bad at holding a long logical chain without
quietly contradicting itself. elenchus moves the logic out of the model: you
state only **facts** and **first principles (axioms)**; a Rust engine does the
inference and finds contradictions mathematically. You can only be wrong at the
axiom level — and that is caught immediately.

Three truth values (this is the whole epistemic trick): an atom is **TRUE** (you
wrote `FACT`), **FALSE** (you wrote `NOT`), or **UNKNOWN** (you didn't mention
it). UNKNOWN is *not* false — the engine never guesses.

## The loop — this is the point

Running once is not the job. The verdict tells you what to do next; you iterate
until it is **CONSISTENT**.

```
   write facts + axioms ─▶ run ─▶ CONSISTENT?  ── yes ─▶ done (exit 0)
            ▲                         │ no
            │                         ▼
            └──── add facts / fix or rethink an axiom ◀── read the verdict
```

| Verdict | exit | It means | Your next move |
|--------|:----:|----------|----------------|
| **CONSISTENT** | 0 | no contradiction, answer is pinned down | done |
| **WARNING** | 1 | an axiom can't be checked — a needed atom is UNKNOWN | add the `FACT`/`NOT` it names under `blocked by:` |
| **UNDERDETERMINED** | 1 | satisfiable, but several models fit | add the fact it suggests (`pin it down: add …`) |
| **CONFLICT** | 2 | an axiom is violated, or the axioms are jointly unsatisfiable | a fact is wrong, or two principles can't both hold — fix one |

So: **WARNING / UNDERDETERMINED / CONFLICT are never "done"** — they are the
engine telling you exactly what is missing or wrong. Re-run after each change.

## DSL cheat-sheet

Keywords are ALWAYS CAPS, names are lowercase, indentation is cosmetic, `//` is a
comment. An atom is `Subject predicate [object]`.

| Form | Meaning |
|------|---------|
| `FACT s p [o]` | assert it TRUE |
| `NOT s p [o]` | assert it FALSE |
| `AXIOM n:` → `EXCLUSIVE` *(≥2 atoms)* | **at most one** is TRUE |
| `AXIOM n:` → `FORBIDS` | at most one is TRUE (for two: "not both") |
| `AXIOM n:` → `ONEOF` | **exactly one** is TRUE |
| `AXIOM n:` → `ATLEAST` | **at least one** is TRUE |
| `AXIOM n:` → `WHEN a` `AND b` `THEN c` | if `a ∧ b` hold then `c` must hold (else CONFLICT) |
| `RULE n:` → `WHEN … THEN c` | same shape, but **derives** `c` as a new fact |
| `IMPORT "lib.vrf"` | merge a reusable axiom library (atoms unify across files) |
| `CHECK [s] [BIDIRECTIONAL]` | run it; `BIDIRECTIONAL` also searches for alternative models (UNDERDETERMINED) |

Numbers? Turn a condition into a named boolean atom (`speed >= 100` → `over_100`)
and reason about the branch, not the arithmetic.

## Deeper mechanics (condensed from the spec)

**One primitive.** Every constraint compiles to `Impossible([…])` — "these
literals can't all be TRUE at once". `EXCLUSIVE`/`ONEOF`/`ATLEAST`/`FORBIDS` and
`WHEN…THEN` are sugar over it. You never write `Impossible`; this is just why the
engine stays small and total.

**Atom identity is verbatim — the #1 gotcha.** Atoms are compared
character-for-character, case-sensitively, by the triple `(subject, predicate,
object)`. `has_fuel` ≠ `hasFuel` ≠ `Has_fuel`. A typo or alternate spelling
silently creates a *new* UNKNOWN atom — so an axiom about `Engine has fuel` will
not see your `Engine has_fuel`. Keep names identical across facts, axioms, and
imports, or you'll get phantom WARNINGs.

**WHEN…THEN: why WARNING vs CONFLICT.** For `WHEN A AND B THEN C`:

- any antecedent FALSE → the rule doesn't fire → CONSISTENT (not a warning);
- all antecedents TRUE and C FALSE → **CONFLICT**;
- all TRUE with C UNKNOWN, or an antecedent UNKNOWN → **WARNING** (state more);
- a `RULE` instead *derives* C = TRUE when the antecedent holds.

List axioms (`EXCLUSIVE`/…) with UNKNOWN atoms stay CONSISTENT — no data, no
conflict yet.

**Forward vs backward.** The forward pass checks the facts you gave and runs
`RULE`s. `CHECK X BIDIRECTIONAL` adds a backward pass (a small SAT search): it
reports **UNDERDETERMINED** when more than one model fits, and catches a
**CONFLICT** where the axioms are jointly unsatisfiable even though no single one
is visibly violated. Use `BIDIRECTIONAL` when you care "is the answer *unique*?".

**IMPORT.** `IMPORT "lib.vrf"` flat-merges another source; atoms **unify across
files** (a library axiom constrains your local fact). Axiom *names* are per-source
labels — two files may reuse a name; identical duplicates are idempotent.

**Not supported (on purpose).** No arithmetic (turn a number into a boolean atom),
no quantifiers (∀/∃), no probabilities. Pure boolean structure — exactly the class
of mistakes a model makes across a long chain.

## Patterns (recipes)

- **Exactly one of N** → `ONEOF`. (Each person one role; a request is exactly one of pending/done/failed.)
- **Mutually exclusive** → `EXCLUSIVE` / `FORBIDS`. (Can't be both `fast_path` and `slow_path`.)
- **Required together / gate** → `WHEN … THEN …`. (Deploy only when built ∧ tested ∧ reviewed.)
- **At least one** → `ATLEAST`. (At least one reviewer; at least one branch taken.)
- **Ordering / implication between thresholds** → `WHEN over_200 THEN over_100`.
- **Derive consequences** → `RULE`. (If `flying`, derive `needs oxygen`.)
- **Shared first principles** → `IMPORT` a vetted library; you write only the `FACT`s.

## Worked examples (note the loop)

### 1. Exactly-one assignment (deduced by the engine)

```vrf
AXIOM alice_role:
    ONEOF
        alice is lead
        alice is dev
        alice is qa
// ...bob_role, carol_role, and lead_one / dev_one / qa_one the same way...
FACT alice is lead
NOT  bob is qa
CHECK alice BIDIRECTIONAL
```

`CONSISTENT` — from the clues the engine deduced bob=dev, carol=qa by case
analysis. Remove `NOT bob is qa` → `UNDERDETERMINED` (bob/carol could swap); add a
clue to pin it. Assert two leads → `CONFLICT`.

### 2. A deploy gate — iterate from WARNING to CONSISTENT

```vrf
FACT svc built
FACT svc unit_tested
NOT  svc deprecated
AXIOM ready_needs_all:
    WHEN svc built
    AND  svc integration_tested
    AND  svc security_scanned
    THEN svc release_ready
AXIOM can_deploy:
    WHEN svc release_ready
    AND  NOT svc deprecated
    THEN svc can_deploy
CHECK svc
```

Run → `WARNING` (`blocked by: svc integration_tested, svc security_scanned`).
You haven't stated enough. Add `FACT svc integration_tested`, `FACT svc
security_scanned`, `FACT svc release_ready`, `FACT svc can_deploy` → re-run →
`CONSISTENT`. But if you assert `FACT svc release_ready` with `NOT svc
can_deploy`, the `can_deploy` axiom fires → `CONFLICT` (fix the fact).

### 3. Branch coverage — find a bug with no numbers

```vrf
FACT Motor over_100
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
CHECK Motor
```

`CONSISTENT` (derives `uses fast_path`). If your logic could ever take both
paths, `one_path` reports `CONFLICT` — a branch bug caught structurally.

### 4. Jointly-unsatisfiable axioms — only the backward pass finds it

```vrf
AXIOM a_to_b:
    WHEN x a
    THEN x b
AXIOM a_to_not_b:
    WHEN x a
    THEN NOT x b
AXIOM need_a_or_c:
    ATLEAST
        x a
        x c
AXIOM c_to_a:
    WHEN x c
    THEN x a
CHECK x BIDIRECTIONAL
```

No single clause is violated (everything is UNKNOWN), but the axioms can't all
hold (c→a→both b and ¬b). With `BIDIRECTIONAL`, elenchus reports `CONFLICT`:
"the axioms and facts are jointly unsatisfiable." Rethink the principles.

## How to use it (CLI or MCP — same engine)

This skill is the instructions; the engine is reachable two interchangeable ways.
Use whichever your environment provides — both are cross-platform:

**CLI + this skill — preferred when you have a shell** (the playwright-cli + skill
shape). Run the `elenchus` binary via Bash. One portable binary, zero host config.

```console
$ elenchus program.vrf                  # check a file (IMPORTs resolve next to it)
$ elenchus --text "FACT x a
CHECK x" --format json                  # inline; --format json for tooling
$ cat program.vrf | elenchus            # stdin
```

Exit code is the verdict (0 = CONSISTENT, 1 = WARNING/UNDERDETERMINED,
2 = CONFLICT) — a ready CI gate.

**MCP + this skill — when the host exposes MCP tools but no shell.** The
`elenchus-mcp` server provides one tool, `elenchus_check`; call it with
`{ "program": "<.vrf text>", "format": "json" }`.

Either way the rule is the same: read `status`; if it isn't `CONSISTENT`, change
the program (add facts, fix or rethink an axiom) and run again — **loop until it
is CONSISTENT**.
