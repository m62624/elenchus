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
state only **facts** and **first principles (premises)**; a Rust engine does the
inference and finds contradictions mathematically. You can only be wrong at the
premise level — and that is caught immediately.

**What this actually is: a simplified SAT checker.** SAT = boolean
*satisfiability*: given a pile of TRUE/FALSE constraints, find an assignment of
values that satisfies all of them at once, or prove that none exists. That's the
whole engine. elenchus adds one thing — a third truth value, **UNKNOWN** — and
reports the outcome as CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT instead
of a raw sat/unsat. It does **not** do arithmetic or proofs about numbers (that's
SMT, a bigger tool) — only boolean structure.

Three truth values: an atom is **TRUE** (you wrote `FACT`), **FALSE** (you wrote
`NOT`), or **UNKNOWN** (you didn't mention it). UNKNOWN is *not* false — the engine
never guesses.

## The loop — this is the point

Running once is not the job. The verdict tells you what to do next; you iterate
until it is **CONSISTENT**.

```
   write facts + premises ─▶ run ─▶ CONSISTENT?  ── yes ─▶ done (exit 0)
            ▲                         │ no
            │                         ▼
            └──── add facts / fix or rethink a premise ◀── read the verdict
```

| Verdict | exit | It means | Your next move |
|--------|:----:|----------|----------------|
| **CONSISTENT** | 0 | no contradiction, answer is pinned down | done |
| **WARNING** | 1 | a premise can't be checked — a needed atom is UNKNOWN | add the `FACT`/`NOT` it names under `blocked by:` |
| **UNDERDETERMINED** | 1 | satisfiable, but several models fit | add the fact it suggests (`pin it down: add …`) |
| **CONFLICT** | 2 | a premise is violated, or the premises are jointly unsatisfiable | a fact is wrong, or two principles can't both hold — fix one |

So: **WARNING / UNDERDETERMINED / CONFLICT are never "done"** — they are the
engine telling you exactly what is missing or wrong. Re-run after each change.

**Reach CONSISTENT by stating more truth, not by gaming the check.** CONSISTENT
only certifies "no contradiction *among what you wrote*" — it cannot vouch for
constraints you never stated. So the goal of the loop is a **faithful, complete
model**, not just a green result:

- When you hit WARNING/UNDERDETERMINED, add the **real** missing `FACT`/`NOT` — not
  an invented one. If you don't actually know it, that uncertainty is the finding.
- When you hit CONFLICT, fix the thing that is actually wrong. **Never delete a
  valid premise or assert something false just to make it pass** — that throws away
  the very check you wanted.
- Before each run, ask: *have I encoded every constraint that matters here?* Add
  every genuine first principle as an `PREMISE` (mutual exclusions, required-together
  gates, orderings, "exactly one of"). A missing premise = false confidence: the
  engine will happily say CONSISTENT about an under-specified problem.

Done = CONSISTENT **with every real premise present and every known fact stated** —
no remaining "but…". Keep iterating (add data, add premises) until you reach that.

## DSL cheat-sheet

Keywords are ALWAYS CAPS (ASCII). Names are content — **case-sensitive and matched
verbatim** (`has_fuel` ≠ `hasFuel`) — and may use letters of **any script**
(`условие`, `名前` are fine; `snake_case` is just convention). Join multi-word names
with `_` (no spaces inside one name). Indentation is cosmetic, `//` is a comment.
An atom is `Subject predicate [object]`.

| Form | Meaning |
|------|---------|
| `FACT s p [o]` | assert it TRUE |
| `NOT s p [o]` | assert it FALSE |
| `PREMISE n:` → `EXCLUSIVE` *(≥2 atoms)* | **at most one** is TRUE |
| `PREMISE n:` → `FORBIDS` | at most one is TRUE (for two: "not both") |
| `PREMISE n:` → `ONEOF` | **exactly one** is TRUE |
| `PREMISE n:` → `ATLEAST` | **at least one** is TRUE |
| `PREMISE n:` → `WHEN a` `AND b` `THEN c` | if `a ∧ b` hold then `c` must hold (else CONFLICT) |
| `RULE n:` → `WHEN … THEN c` | same shape, but **derives** `c` as a new fact |
| `IMPORT "lib.vrf"` | merge a reusable premise library (atoms unify across files) |
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
silently creates a *new* UNKNOWN atom — so a premise about `Engine has fuel` will
not see your `Engine has_fuel`. Keep names identical across facts, premises, and
imports, or you'll get phantom WARNINGs.

**WHEN…THEN: why WARNING vs CONFLICT.** For `WHEN A AND B THEN C`:

- any antecedent FALSE → the rule doesn't fire → CONSISTENT (not a warning);
- all antecedents TRUE and C FALSE → **CONFLICT**;
- all TRUE with C UNKNOWN, or an antecedent UNKNOWN → **WARNING** (state more);
- a `RULE` instead *derives* C = TRUE when the antecedent holds.

List premises (`EXCLUSIVE`/…) with UNKNOWN atoms stay CONSISTENT — no data, no
conflict yet.

**Forward vs backward.** The forward pass checks the facts you gave and runs
`RULE`s. `CHECK X BIDIRECTIONAL` adds a backward pass (a small SAT search): it
reports **UNDERDETERMINED** when more than one model fits, and catches a
**CONFLICT** where the premises are jointly unsatisfiable even though no single one
is visibly violated. Use `BIDIRECTIONAL` when you care "is the answer *unique*?".

**IMPORT.** `IMPORT "lib.vrf"` flat-merges another source. Think of it like a
**linker**, not like importing a function: there is nothing to "call" or
"override". Files link through **shared atoms** — `Engine.X has fuel` means the
same atom in every file, so an imported premise automatically constrains your
local `FACT` about that atom. Premise *names* are per-source **labels** (for the
report only), not global identifiers: two files may reuse a name with no clash,
and you never reference a premise by name. Consequences worth knowing:
- Imports **compose by conjunction** — you get the AND of all premises. There is
  no precedence and no overriding; if two files genuinely disagree
  (`A→B` vs `A→¬B`) that is a real **CONFLICT** and the engine surfaces it (that's
  the point — it won't silently pick a winner).
- Identical premises/facts (same content) dedupe; transitive and diamond imports
  merge once; cycles are rejected.
- Want two domains that should *not* link? Don't share atom names — namespace the
  **subject** (`fantasy.bird has wings` vs `bio.bird has wings`). Separation lives
  in atom names, not in premise names.

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
PREMISE alice_role:
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
PREMISE ready_needs_all:
    WHEN svc built
    AND  svc integration_tested
    AND  svc security_scanned
    THEN svc release_ready
PREMISE can_deploy:
    WHEN svc release_ready
    AND  NOT svc deprecated
    THEN svc can_deploy
CHECK svc
```

Run → `WARNING` (`blocked by: svc integration_tested, svc security_scanned`).
You haven't stated enough. Add `FACT svc integration_tested`, `FACT svc
security_scanned`, `FACT svc release_ready`, `FACT svc can_deploy` → re-run →
`CONSISTENT`. But if you assert `FACT svc release_ready` with `NOT svc
can_deploy`, the `can_deploy` premise fires → `CONFLICT` (fix the fact).

### 3. Branch coverage — find a bug with no numbers

```vrf
FACT Motor over_100
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
CHECK Motor
```

`CONSISTENT` (derives `uses fast_path`). If your logic could ever take both
paths, `one_path` reports `CONFLICT` — a branch bug caught structurally.

### 4. Jointly-unsatisfiable premises — only the backward pass finds it

```vrf
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

No single clause is violated (everything is UNKNOWN), but the premises can't all
hold (c→a→both b and ¬b). With `BIDIRECTIONAL`, elenchus reports `CONFLICT` and
prints a **`CORE`** — the minimal set of premises jointly to blame, so you know
exactly which principles to revisit:

```
  CONFLICT  - (UNSAT)  [<system>:0]
      the premises and facts are jointly unsatisfiable
  CORE  smallest jointly-unsatisfiable set (4):
        a_to_b (PREMISE) [..]   a_to_not_b (PREMISE) [..]
        need_a_or_c (ATLEAST) [..]   c_to_a (PREMISE) [..]
```

### 5. A puzzle solved by deduction (the SAT pass at work)

You give constraints, not the answer; the backward (SAT) pass deduces it and
proves it's the only one. (Full file: `docs/examples/roles-puzzle.vrf`.)

```vrf
// 3 people × 3 roles. Each person ONEOF its 3 roles; each role ONEOF the 3 people.
PREMISE alice_one_role:
    ONEOF
        alice is lead
        alice is dev
        alice is qa
// ...bob_one_role, carol_one_role, and lead_one_person/dev_one_person/qa_one_person
//    the same shape (each ONEOF over its three atoms)...
FACT alice is lead       // clue 1
NOT  bob is qa           // clue 2
CHECK BIDIRECTIONAL
```

`CONSISTENT` — from just two clues the engine deduces bob=dev, carol=qa and proves
that assignment is **unique**. Drop `NOT bob is qa` → `UNDERDETERMINED` (bob/carol
can swap). Add `FACT bob is lead` (two leads) → `CONFLICT`. This is the payoff of
"simplified SAT": you state the rules, the engine does the case analysis you'd
otherwise get wrong by hand.

## Reading a CONFLICT: the `why:` trace

A `CONFLICT` is not a dead end — the report shows **why**. For a violated premise
it prints the derivation chain that forced the clashing atoms (supporting facts
first, then each rule built on them), so you can see the exact step that's wrong:

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

Read it top-down: `socrates is immortal` (a fact) collides with `socrates is
mortal`, which the chain derived from `socrates is human`. Fix one link — drop the
fact, or reject a rule. (Jointly-unsatisfiable systems print a `CORE` instead, as
in example 4.) Both the human report and the JSON carry this (`trace`,
`unsat_core`).

## How to use it (CLI or MCP — same engine)

The engine is reachable two interchangeable ways. **First decide which one you
have here, then smoke-test it — don't trust an engine you haven't confirmed runs
on this machine.**

**Step 0 — detect what's installed.**

- If you can run shell commands → use the **CLI**. Confirm it's there:
  `elenchus --version` prints a version line if installed; "command not found"
  means it isn't.
- If you have no shell but your available tools include an `elenchus_check` tool →
  use **MCP**.
- If neither: it isn't installed here. Install it (see the project README —
  `cargo binstall elenchus-cli`, Homebrew, the Windows `.msi`, or the curl/irm
  script), then re-check. Don't fabricate verdicts; if you can't run it, say so.

**Step 0.5 — smoke-test it.** Run one tiny program whose answer you already know,
and confirm the verdict before relying on the engine for real work:

```console
$ elenchus --text "FACT x a
NOT x a
CHECK x"
RESULT: CONFLICT          # TRUE and FALSE on the same atom MUST be a CONFLICT
EXIT_CODE: 2              # 2 = CONFLICT
```

`CONFLICT` / exit 2 means the engine is healthy. (Via MCP: call `elenchus_check`
with that same `program` and check `status` == `"CONFLICT"`.) If it answers
anything else, the install is broken — fix that before trusting any result.

**Step 0.6 — version check (don't skip).**
<!-- skill-version: 0.3.0 -->
This skill documents elenchus **0.3.0**. You already learned the installed
version (`elenchus --version`, or the MCP `initialize` response's
`serverInfo.version`). Compare it to **0.3.0**:

- **Match** → proceed normally.
- **Mismatch** → STOP and warn the user before relying on any flag or output
  described below, e.g. *"⚠️ version skew: this skill targets elenchus 0.3.0 but
  the engine reports 0.2.1."* Then say which side is **newer** and what it implies:
  - skill **newer** than the binary → the binary is outdated; features described
    here (the `why:` trace, `CORE`, JSON `trace`/`unsat_core`) may be missing →
    suggest updating elenchus.
  - binary **newer** than the skill → this skill is stale; flags or output may
    have changed → suggest updating the skill.
  You may still run the engine, but mark every result as "verify — version skew".

**CLI + this skill — preferred when you have a shell.** Run the `elenchus` binary
via Bash. One portable binary, zero host config.

```console
$ elenchus program.vrf                  # check a file (IMPORTs resolve next to it)
$ elenchus --text "FACT x a
CHECK x" --format json                  # inline; --format json for tooling
$ cat program.vrf | elenchus            # stdin
```

It takes one input three ways — a positional `<file>`, inline `--text "..."`, or
stdin (no arg, or `-`). `--text` and a file are mutually exclusive (don't pass
both). **`IMPORT` only resolves in the file form** (relative to the file);
`--text` and stdin are treated as a single source, so a program that uses
`IMPORT` must be run as a file. Exit code is the verdict (0 = CONSISTENT,
1 = WARNING/UNDERDETERMINED, 2 = CONFLICT) — a ready CI gate.

**MCP + this skill — when the host exposes MCP tools but no shell.** The
`elenchus-mcp` server provides one tool, `elenchus_check`; call it with
`{ "program": "<.vrf text>", "format": "json" }`. It's one source too — no
`IMPORT` resolution; inline the premises instead.

Either way the rule is the same: read `status`; if it isn't `CONSISTENT`, change
the program (add facts, fix or rethink a premise) and run again — **loop until it
is CONSISTENT**.
