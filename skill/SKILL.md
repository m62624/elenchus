---
name: elenchus
description: >-
  Verify that a set of facts and constraints is logically consistent — catch
  contradictions, missing data, and under-specified problems before they bite.
  Reach for this whenever correctness depends on a web of conditions a model is
  likely to lose track of: role/shift/seat assignments, "exactly one of these",
  mutually-exclusive states, deploy/readiness gates, if/else branch coverage,
  state-machine legality, invariants across many flags. You write the facts and
  first principles in a tiny English-like DSL; the elenchus engine does all the
  logic and tells you CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT.
---

# elenchus — a logical-consistency checker

A small model is good at stating facts but bad at holding a 7-step logical chain
without quietly contradicting itself. **elenchus** moves the logic out of the
model: you write only **facts** and **first principles (axioms)** in a tiny DSL,
and a Rust engine does the inference and catches contradictions mathematically.
You can only err at the axiom level — and that error is caught early.

## When to use it

Use it whenever a result is "correct" only if many conditions hold together and
it's easy to miss one:

- assignments / permutations — *each person gets exactly one role*, *each seat one person*;
- mutually-exclusive states — *a request is `pending`, `done`, or `failed`, never two*;
- readiness / deploy gates — *deploy only if built ∧ tested ∧ reviewed ∧ not deprecated*;
- code branch coverage — *every branch of an `if/else` chain is handled and reachable*;
- state machines — *which transitions are legal from here*;
- invariants over many boolean flags where a contradiction would be silent.

If the question is numeric (`x > 100`), turn the condition into a named boolean
atom (`over_100`) — elenchus reasons about the **branch**, not the arithmetic.

## The DSL in one minute

Keywords are ALWAYS CAPS; names are lowercase; indentation is cosmetic.

```vrf
FACT  Subject predicate [object]      // an assertion that is TRUE
NOT   Subject predicate [object]      // an assertion that is FALSE
// anything not stated is UNKNOWN (not false!)

AXIOM name:                           // a first principle — CHECKED
    EXCLUSIVE                         // at most one is TRUE   (also FORBIDS, ONEOF, ATLEAST)
        Subject predicate
        Subject predicate
AXIOM name:
    WHEN  Subject predicate           // implication: if all the WHENs/ANDs hold...
    AND   Subject predicate
    THEN  Subject predicate           // ...then this must hold (violation → CONFLICT)

RULE name:                            // like an AXIOM body, but DERIVES a new fact
    WHEN  Subject predicate
    THEN  Subject predicate

IMPORT "library.vrf"                  // reuse a vetted axiom library (atoms unify across files)

CHECK Subject [BIDIRECTIONAL]         // run the check (BIDIRECTIONAL also looks for alternatives)
```

## The four results (and what to do)

| Result | exit | Meaning | What to do |
|--------|------|---------|------------|
| **CONSISTENT** | 0 | no contradictions, pinned down | done |
| **WARNING** | 1 | an axiom couldn't be checked — a needed atom is UNKNOWN | add the missing `FACT`/`NOT` |
| **UNDERDETERMINED** | 1 | satisfiable, but more than one model fits | add a fact (the report names one) to pin it |
| **CONFLICT** | 2 | an axiom is violated, or the facts+axioms are jointly unsatisfiable | fix a fact, or rethink the axiom |

**This is the core workflow — do not stop at the first run.** WARNING,
UNDERDETERMINED and CONFLICT are *not* "done". They are the engine telling you
what is still missing or wrong:

1. Run the check.
2. If **CONFLICT** — a first principle is broken. Either a fact is wrong, or two
   axioms can't both hold. Fix it.
3. If **WARNING** — you haven't stated enough. Add the `FACT`/`NOT` the report
   lists under `blocked by:`.
4. If **UNDERDETERMINED** — the constraints don't determine a unique answer. Add
   the fact the report suggests (`pin it down: add FACT … or NOT …`).
5. Re-run. Iterate until **CONSISTENT (exit 0)**.

## Worked examples

### Real life — exactly-one role assignment

Alice/Bob/Carol each take exactly one of lead/dev/qa; each role goes to one person.

```vrf
AXIOM alice_role:
    ONEOF
        alice is lead
        alice is dev
        alice is qa
// ... bob_role, carol_role, and lead_one/dev_one/qa_one the same way ...

FACT alice is lead
NOT  bob is qa
CHECK alice BIDIRECTIONAL
```

→ `CONSISTENT` — the engine deduced bob=dev, carol=qa by case analysis. Drop the
`NOT bob is qa` clue and it becomes `UNDERDETERMINED` (bob/carol could swap):
add a fact to pin it. Make two people `lead` and it's `CONFLICT`.

### Code — deploy gate and branch coverage

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

→ `WARNING` (blocked by `svc integration_tested`, `svc security_scanned`): you
asserted the build/test but not the rest of the gate. Add those facts (or `NOT`
them) and re-run. If you assert `svc release_ready` but `NOT svc can_deploy`,
the `can_deploy` axiom fires → `CONFLICT`.

### Code — mutually-exclusive branches

```vrf
// if speed >= 100 { fast } else { slow }   →   model the branch as an atom
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

If both paths can end up taken, `one_path` reports `CONFLICT` — you found a branch
bug without a single number.

## How to invoke

**CLI** (`elenchus`):

```console
$ elenchus program.vrf                 # check a file (IMPORTs resolve relative to it)
$ elenchus --text "FACT x a
CHECK x" --format json                 # inline; --format json for tooling
$ cat program.vrf | elenchus           # stdin
```

Exit code is the verdict (0 / 1 / 2) — usable directly as a CI gate.

**MCP tool** (`elenchus-mcp` server): call the `elenchus_check` tool with
`{ "program": "<.vrf text>", "format": "json" }`. The result text is the report;
keep refining the program and calling again until `status` is `CONSISTENT`.
