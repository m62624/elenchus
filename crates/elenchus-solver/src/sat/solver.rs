//! The CDCL search core: trail + decision levels, two-watched-literal
//! propagation, 1-UIP conflict analysis with learning, and VSIDS decisions.
use alloc::vec;
use alloc::vec::Vec;

use super::{Budget, BudgetExhausted, Cnf, SatLit, SolverConfig, Stats, Var};

/// How a [`Solver::run`] ended without a model.
pub(crate) enum RunFail {
    /// Unsatisfiable, with a sufficient subset of the assumptions (empty when
    /// UNSAT regardless of them). A real, terminal answer.
    Unsat(Vec<SatLit>),
    /// The conflict [`Budget`] ran out mid-search — no answer. Only reachable
    /// when a budget is installed.
    Exhausted,
}

/// Why a variable was assigned — needed for conflict analysis and backtracking.
#[derive(Clone, Copy)]
enum Reason {
    Decision,
    Unit,
    Long(usize),
}

/// One watched-literal entry: a clause plus a cached "other" literal so a true
/// blocking literal lets us skip the clause entirely.
#[derive(Clone, Copy)]
struct Watch {
    cref: usize,
    blocking: SatLit,
}

/// A variable's slot in [`VarOrder::pos`] when it is not on the heap.
const NOT_IN_HEAP: usize = usize::MAX;

/// MiniSat-style variable-order heap: a binary max-heap over variables keyed by
/// `(activity desc, index asc)`, with a position index so a bumped variable can
/// sift up in place. The tie-break replicates the linear argmax scan this
/// replaced **exactly** (first = lowest-index variable among equal activities),
/// so the decision sequence — and therefore every model, core, and report — is
/// byte-identical; only the cost changes, O(log n) per decision instead of a
/// full O(n) scan (which dominated on programs whose atoms are mostly
/// unconstrained, since each must still be decided to complete a model).
///
/// Invariant: every unassigned variable is on the heap. Popped variables that
/// turn out assigned are simply discarded; [`Solver::backtrack`] re-inserts
/// whatever it unassigns.
struct VarOrder {
    heap: Vec<Var>,
    pos: Vec<usize>, // var -> index in `heap`, or NOT_IN_HEAP
}

impl VarOrder {
    /// All variables start at zero activity, so plain index order already
    /// satisfies the heap property under the tie-break.
    fn new(n: usize) -> Self {
        VarOrder {
            heap: (0..n as Var).collect(),
            pos: (0..n).collect(),
        }
    }

    /// Strict priority order: higher activity first, lower index on ties.
    fn less(activity: &[f64], a: Var, b: Var) -> bool {
        let (aa, ab) = (activity[a as usize], activity[b as usize]);
        aa > ab || (aa == ab && a < b)
    }

    fn contains(&self, v: Var) -> bool {
        self.pos[v as usize] != NOT_IN_HEAP
    }

    fn sift_up(&mut self, activity: &[f64], mut i: usize) {
        let v = self.heap[i];
        while i > 0 {
            let p = (i - 1) >> 1;
            if Self::less(activity, v, self.heap[p]) {
                self.heap[i] = self.heap[p];
                self.pos[self.heap[i] as usize] = i;
                i = p;
            } else {
                break;
            }
        }
        self.heap[i] = v;
        self.pos[v as usize] = i;
    }

    fn sift_down(&mut self, activity: &[f64], mut i: usize) {
        let v = self.heap[i];
        loop {
            let l = 2 * i + 1;
            if l >= self.heap.len() {
                break;
            }
            let r = l + 1;
            let c = if r < self.heap.len() && Self::less(activity, self.heap[r], self.heap[l]) {
                r
            } else {
                l
            };
            if Self::less(activity, self.heap[c], v) {
                self.heap[i] = self.heap[c];
                self.pos[self.heap[i] as usize] = i;
                i = c;
            } else {
                break;
            }
        }
        self.heap[i] = v;
        self.pos[v as usize] = i;
    }

    /// Register a brand-new highest-index variable and put it on the heap. At
    /// zero activity it sorts after every existing zero-activity variable (the
    /// index tie-break), exactly where the old linear scan would have found it.
    fn push_var(&mut self, activity: &[f64], v: Var) {
        debug_assert_eq!(self.pos.len(), v as usize);
        self.pos.push(NOT_IN_HEAP);
        self.insert(activity, v);
    }

    /// Put `v` (back) on the heap; a no-op if it is already there.
    fn insert(&mut self, activity: &[f64], v: Var) {
        if self.contains(v) {
            return;
        }
        self.pos[v as usize] = self.heap.len();
        self.heap.push(v);
        self.sift_up(activity, self.heap.len() - 1);
    }

    /// Remove and return the highest-priority variable, or `None` when empty.
    fn pop(&mut self, activity: &[f64]) -> Option<Var> {
        let last = self.heap.pop()?;
        if self.heap.is_empty() {
            self.pos[last as usize] = NOT_IN_HEAP; // `last` was the root itself
            return Some(last);
        }
        let top = core::mem::replace(&mut self.heap[0], last);
        self.pos[top as usize] = NOT_IN_HEAP;
        self.pos[last as usize] = 0;
        self.sift_down(activity, 0);
        Some(top)
    }

    /// Restore heap order after `v`'s activity increased (a VSIDS bump). A
    /// uniform rescale of all activities needs nothing — order is unchanged.
    fn bumped(&mut self, activity: &[f64], v: Var) {
        if self.contains(v) {
            let i = self.pos[v as usize];
            self.sift_up(activity, i);
        }
    }
}

/// What the decision phase produced. The search loop reacts to each.
enum Decision {
    /// A literal (an assumption or a VSIDS branch) was enqueued; propagate next.
    Propagated,
    /// Every variable is assigned under the assumptions — satisfiable.
    Sat,
    /// An assumption is contradicted; carries a sufficient core (a subset of the
    /// assumptions). An empty core means UNSAT independent of the assumptions.
    UnsatCore(Vec<SatLit>),
}

/// The full CDCL search state: the assignment trail with decision levels, the
/// clause database with two-watched-literal indices, VSIDS activities with phase
/// saving, and a reusable `seen` scratch buffer for conflict analysis.
pub(crate) struct Solver {
    clauses: Vec<Vec<SatLit>>, // originals + learned + blocking
    watches: Vec<Vec<Watch>>, // indexed by literal code; a clause watching `w` lives in watches[!w]
    assign: Vec<Option<bool>>, // per var
    level: Vec<u32>,          // per var (valid when assigned)
    reason: Vec<Reason>,      // per var (valid when assigned)
    trail: Vec<SatLit>,
    decisions: Vec<usize>, // trail index where each decision level starts
    qhead: usize,
    activity: Vec<f64>,
    var_inc: f64,
    order: VarOrder,     // decision queue: every unassigned var is on it
    polarity: Vec<bool>, // phase saving
    seen: Vec<bool>,     // reusable scratch for analyze (invariant: all-false between calls)
    touched: Vec<Var>,   // reusable scratch for analyze (invariant: empty between calls)
    ok: bool,            // false once the formula is known UNSAT
    // Literals forced true before VSIDS branching. Decision levels 1..=len map
    // one-to-one to assumptions[0..]; an already-true assumption still consumes a
    // (dummy) level so that mapping holds. Empty for a plain solve.
    pub(crate) assumptions: Vec<SatLit>,
    stats: Stats, // deterministic work counters (never reset over the lifetime)
    config: SolverConfig,
    // Shared conflict pool; `None` (the default) can never abort a solve.
    budget: Option<Budget>,
}

impl Solver {
    /// Build a solver and load every clause of `cnf` under the empty assignment,
    /// with the reference (heuristics-off) profile.
    pub(crate) fn new(cnf: &Cnf) -> Self {
        Self::with_config(cnf, SolverConfig::default())
    }

    /// Like [`Solver::new`] with an explicit heuristics profile.
    pub(crate) fn with_config(cnf: &Cnf, config: SolverConfig) -> Self {
        let n = cnf.num_vars;
        let mut s = Solver {
            clauses: Vec::new(),
            watches: vec![Vec::new(); 2 * n],
            assign: vec![None; n],
            level: vec![0; n],
            reason: vec![Reason::Decision; n],
            trail: Vec::new(),
            decisions: Vec::new(),
            qhead: 0,
            activity: vec![0.0; n],
            var_inc: 1.0,
            order: VarOrder::new(n),
            polarity: vec![false; n],
            seen: vec![false; n],
            touched: Vec::new(),
            ok: true,
            assumptions: Vec::new(),
            stats: Stats::default(),
            config,
            budget: None,
        };
        for clause in &cnf.clauses {
            s.add_clause(clause);
        }
        s
    }

    // -- assignment queries --

    /// Is `l` currently assigned true? (Unassigned counts as neither true nor false.)
    fn lit_is_true(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(!l.is_negative())
    }
    /// Is `l` currently assigned false?
    fn lit_is_false(&self, l: SatLit) -> bool {
        self.assign[l.var() as usize] == Some(l.is_negative())
    }
    /// The current decision level (= number of open decisions).
    fn current_level(&self) -> u32 {
        self.decisions.len() as u32
    }

    // -- clause loading --

    /// Register clause `cref` to be watched by literals `a` and `b`. A clause
    /// watching a literal is stored under that literal's *negation's* code, so
    /// it is revisited exactly when the watched literal becomes false.
    fn watch(&mut self, cref: usize, a: SatLit, b: SatLit) {
        self.watches[a.negate().code()].push(Watch { cref, blocking: b });
        self.watches[b.negate().code()].push(Watch { cref, blocking: a });
    }

    /// Attach a clause under the *current* assignment. Both watched literals must
    /// be non-false, or the clause is unit/conflicting and is handled directly.
    /// This is what makes incremental clause addition (blocking clauses added
    /// mid-search, at level 0) correct — naively watching `lits[0..2]` would break
    /// the invariant when one is already false.
    fn add_clause(&mut self, lits: &[SatLit]) {
        if !self.ok {
            return;
        }
        if lits.is_empty() {
            self.ok = false;
            return;
        }
        if lits.len() == 1 {
            let l = lits[0];
            if self.lit_is_false(l) {
                self.ok = false;
            } else if !self.lit_is_true(l) {
                self.enqueue(l, Reason::Unit);
            }
            return;
        }

        // Find up to two non-false literals to watch.
        let mut clause = lits.to_vec();
        let mut first = None;
        let mut second = None;
        for (i, &l) in clause.iter().enumerate() {
            if !self.lit_is_false(l) {
                if first.is_none() {
                    first = Some(i);
                } else {
                    second = Some(i);
                    break;
                }
            }
        }
        let cref = self.clauses.len();
        match (first, second) {
            // Every literal is false under the current assignment → conflict.
            (None, _) => self.ok = false,
            // Exactly one non-false literal → the clause is unit; assert it.
            (Some(a), None) => {
                clause.swap(0, a);
                self.watch(cref, clause[0], clause[1]);
                let unit = clause[0];
                self.clauses.push(clause);
                if !self.lit_is_true(unit) {
                    self.enqueue(unit, Reason::Long(cref));
                }
            }
            // Two non-false literals → watch them (moved to positions 0 and 1).
            (Some(a), Some(b)) => {
                clause.swap(0, a);
                clause.swap(1, b);
                self.watch(cref, clause[0], clause[1]);
                self.clauses.push(clause);
            }
        }
    }

    /// Assign `l` true at the current level with the given `reason`, and push it
    /// onto the trail for propagation.
    fn enqueue(&mut self, l: SatLit, reason: Reason) {
        let v = l.var() as usize;
        self.assign[v] = Some(!l.is_negative());
        self.level[v] = self.current_level();
        self.reason[v] = reason;
        self.trail.push(l);
    }

    // -- propagation (two-watched-literal) --

    /// Unit-propagate to a fixpoint. Returns the conflicting clause, if any.
    fn propagate(&mut self) -> Option<usize> {
        while self.qhead < self.trail.len() {
            let p = self.trail[self.qhead];
            self.qhead += 1;
            self.stats.propagations = self.stats.propagations.saturating_add(1);
            if let Some(cref) = self.propagate_lit(p) {
                return Some(cref);
            }
        }
        None
    }

    /// Process the clauses watching `!p` after `p` became true.
    fn propagate_lit(&mut self, p: SatLit) -> Option<usize> {
        let fl = p.negate(); // the watched literal that just became false
        let mut ws = core::mem::take(&mut self.watches[p.code()]);
        let mut read = 0;
        let mut write = 0;
        let mut conflict = None;

        while read < ws.len() {
            let w = ws[read];
            read += 1;

            // A satisfied clause (true blocking literal) needs no inspection.
            if self.lit_is_true(w.blocking) {
                ws[write] = w;
                write += 1;
                continue;
            }

            let cref = w.cref;
            if self.clauses[cref][0] == fl {
                self.clauses[cref].swap(0, 1);
            }
            let other = self.clauses[cref][0];
            let kept = Watch {
                cref,
                blocking: other,
            };

            if other != w.blocking && self.lit_is_true(other) {
                ws[write] = kept;
                write += 1;
                continue;
            }

            // Try to move the watch to a non-false unwatched literal.
            if let Some(repl) = self.find_replacement(cref, fl) {
                self.watches[repl.negate().code()].push(kept);
                continue; // watch left this list
            }

            // No replacement: keep watching `fl`; the clause is unit or conflicting.
            ws[write] = kept;
            write += 1;
            if self.lit_is_false(other) {
                while read < ws.len() {
                    ws[write] = ws[read];
                    write += 1;
                    read += 1;
                }
                conflict = Some(cref);
                break;
            }
            self.enqueue(other, Reason::Long(cref));
        }

        ws.truncate(write);
        self.watches[p.code()] = ws;
        conflict
    }

    /// Find a non-false literal in `clause[2..]`, swap it into the watched slot.
    fn find_replacement(&mut self, cref: usize, fl: SatLit) -> Option<SatLit> {
        let len = self.clauses[cref].len();
        for k in 2..len {
            let ck = self.clauses[cref][k];
            if !self.lit_is_false(ck) {
                self.clauses[cref][1] = ck;
                self.clauses[cref][k] = fl;
                return Some(ck);
            }
        }
        None
    }

    // -- conflict analysis (1-UIP) --

    /// VSIDS: raise variable `v`'s activity, rescaling all activities if it would
    /// overflow `f64`'s comfortable range.
    fn bump(&mut self, v: usize) {
        self.activity[v] += self.var_inc;
        if self.activity[v] > 1e100 {
            // A uniform rescale preserves the order, so the heap needs nothing.
            for a in &mut self.activity {
                *a *= 1e-100;
            }
            self.var_inc *= 1e-100;
        }
        self.order.bumped(&self.activity, v as Var);
    }

    /// Learn an asserting clause from `conflict` and return (clause, backjump level).
    /// Uses the reusable `seen`/`touched` buffers and restores both on exit.
    fn analyze(&mut self, conflict: usize) -> (Vec<SatLit>, u32) {
        self.stats.conflicts = self.stats.conflicts.saturating_add(1);
        let cur_level = self.current_level();
        let mut learned: Vec<SatLit> = vec![SatLit(0)]; // slot 0 = asserting literal
        // Borrow the scratch buffer for this call (it is empty on entry/exit), so a
        // long CDCL run reuses one allocation across every conflict instead of
        // allocating a fresh `Vec` per conflict.
        let mut touched: Vec<Var> = core::mem::take(&mut self.touched);
        let mut counter = 0usize;
        let mut idx = self.trail.len();
        let mut start = 0; // conflict clause: scan all; reason clauses: slot 0 is the resolved literal
        let mut confl = conflict;

        let uip = loop {
            for j in start..self.clauses[confl].len() {
                let q = self.clauses[confl][j];
                let v = q.var() as usize;
                if !self.seen[v] && self.level[v] > 0 {
                    self.seen[v] = true;
                    touched.push(v as Var);
                    self.bump(v);
                    if self.level[v] == cur_level {
                        counter += 1;
                    } else {
                        learned.push(q);
                    }
                }
            }
            // The most recently assigned `seen` literal on the trail.
            loop {
                idx -= 1;
                if self.seen[self.trail[idx].var() as usize] {
                    break;
                }
            }
            let lit = self.trail[idx];
            self.seen[lit.var() as usize] = false;
            counter -= 1;
            if counter == 0 {
                break lit; // the sole current-level literal left = the first UIP
            }
            start = 1;
            confl = match self.reason[lit.var() as usize] {
                Reason::Long(c) => c,
                _ => unreachable!("a resolved current-level literal must have a clause reason"),
            };
        };
        learned[0] = uip.negate();
        if self.config.ccmin {
            self.minimize_learned(&mut learned);
        }

        let backjump = self.assertion_level(&mut learned);
        self.var_inc *= 1.0 / 0.95; // VSIDS decay

        for v in touched.drain(..) {
            self.seen[v as usize] = false; // restore the scratch buffer
        }
        self.touched = touched; // give the (now empty) buffer back for next time
        self.stats.learned_literals = self
            .stats
            .learned_literals
            .saturating_add(learned.len() as u64);
        (learned, backjump)
    }

    /// MiniSat's "basic" learned-clause minimization ([`SolverConfig::ccmin`]):
    /// drop `learned[j]` (j ≥ 1) when its reason clause is subsumed by the rest —
    /// every antecedent is already in the learned clause (`seen`, which at this
    /// point in [`Solver::analyze`] marks exactly the `learned[1..]` variables) or
    /// fixed at level 0. Removed literals stay `seen` on purpose: a literal whose
    /// reason rests on another *removed* literal is still redundant (the
    /// implication graph is acyclic, so removals resolve out in reverse trail
    /// order). The asserting literal `learned[0]` is never touched.
    fn minimize_learned(&self, learned: &mut Vec<SatLit>) {
        let mut w = 1;
        for j in 1..learned.len() {
            let v = learned[j].var() as usize;
            let redundant = match self.reason[v] {
                // The reason clause holds v's literal at index 0; antecedents follow.
                Reason::Long(cr) => self.clauses[cr][1..].iter().all(|q| {
                    let qv = q.var() as usize;
                    self.seen[qv] || self.level[qv] == 0
                }),
                _ => false,
            };
            if !redundant {
                learned[w] = learned[j];
                w += 1;
            }
        }
        learned.truncate(w);
    }

    /// Move the highest-level non-asserting literal to index 1 and return its
    /// level (the level to backjump to), or 0 for a unit clause.
    fn assertion_level(&self, learned: &mut [SatLit]) -> u32 {
        if learned.len() == 1 {
            return 0;
        }
        let mut max_i = 1;
        let mut max_l = self.level[learned[1].var() as usize];
        for (i, &lit) in learned.iter().enumerate().skip(2) {
            let l = self.level[lit.var() as usize];
            if l > max_l {
                max_l = l;
                max_i = i;
            }
        }
        learned.swap(1, max_i);
        max_l
    }

    /// MiniSat's `analyzeFinal`. `true_lit` is currently TRUE on the trail and is
    /// the negation of a contradicted assumption; walk its implication graph and
    /// collect the assumptions that entail it. Returns a *sufficient* core — a
    /// subset of [`Solver::assumptions`] (including the contradicted assumption
    /// itself) such that `cnf ∧ core` is unsatisfiable. Restores `seen` on exit.
    fn analyze_final(&mut self, true_lit: SatLit) -> Vec<SatLit> {
        let mut core = vec![true_lit.negate()]; // the contradicted assumption
        if self.current_level() == 0 {
            // `cnf` entails `~assumption` outright; the assumption alone suffices.
            return core;
        }
        let assn = self.assumptions.len() as u32;
        let start = self.decisions[0]; // trail index where level 1 begins
        self.seen[true_lit.var() as usize] = true;
        let mut touched = vec![true_lit.var()];
        let mut i = self.trail.len();
        while i > start {
            i -= 1;
            let x = self.trail[i].var() as usize;
            if !self.seen[x] {
                continue;
            }
            self.seen[x] = false;
            match self.reason[x] {
                // A decision sitting at an assumption level *is* an assumption.
                Reason::Decision => {
                    if self.level[x] > 0 && self.level[x] <= assn {
                        core.push(self.trail[i]);
                    }
                }
                Reason::Unit => {}
                // Pull in the antecedents (clause[1..] are the false literals).
                Reason::Long(cr) => {
                    for j in 1..self.clauses[cr].len() {
                        let v = self.clauses[cr][j].var();
                        if self.level[v as usize] > 0 && !self.seen[v as usize] {
                            self.seen[v as usize] = true;
                            touched.push(v);
                        }
                    }
                }
            }
        }
        for v in touched {
            self.seen[v as usize] = false;
        }
        core
    }

    /// Undo assignments above `level`, saving each unset variable's phase for
    /// later reuse, and rewind the propagation queue to that level.
    fn backtrack(&mut self, level: u32) {
        if self.current_level() <= level {
            return;
        }
        let new_len = self.decisions[level as usize];
        for i in new_len..self.trail.len() {
            let v = self.trail[i].var() as usize;
            self.polarity[v] = self.assign[v] == Some(true);
            self.assign[v] = None;
            self.order.insert(&self.activity, v as Var); // back on the queue
        }
        self.trail.truncate(new_len);
        self.decisions.truncate(level as usize);
        self.qhead = new_len;
    }

    /// Install a freshly learned clause and enqueue its asserting literal.
    fn learn(&mut self, learned: Vec<SatLit>) {
        if learned.len() == 1 {
            self.enqueue(learned[0], Reason::Unit);
        } else {
            let cref = self.clauses.len();
            self.watch(cref, learned[0], learned[1]);
            let assert_lit = learned[0];
            self.clauses.push(learned);
            self.enqueue(assert_lit, Reason::Long(cref));
        }
    }

    // -- decisions --

    /// Choose the next decision: the unassigned variable with the highest VSIDS
    /// activity (lowest index on ties — the [`VarOrder`] tie-break), using its
    /// saved phase. `None` means all variables are assigned: every unassigned
    /// variable is on the heap, so an exhausted heap is a full assignment.
    fn pick_branch(&mut self) -> Option<SatLit> {
        while let Some(v) = self.order.pop(&self.activity) {
            if self.assign[v as usize].is_none() {
                return Some(SatLit::new(v, self.polarity[v as usize]));
            }
        }
        None
    }

    // -- the state machine --

    /// The decision phase: place the next not-yet-satisfied assumption (or detect a
    /// contradicted one and return its core), otherwise branch by VSIDS. Each
    /// assumption — even an already-true one (a dummy level) — consumes exactly one
    /// decision level, so level `i+1` always corresponds to `assumptions[i]`.
    fn decide(&mut self) -> Decision {
        while (self.current_level() as usize) < self.assumptions.len() {
            let p = self.assumptions[self.current_level() as usize];
            if self.lit_is_true(p) {
                self.decisions.push(self.trail.len()); // dummy level, nothing enqueued
            } else if self.lit_is_false(p) {
                return Decision::UnsatCore(self.analyze_final(p.negate()));
            } else {
                self.decisions.push(self.trail.len());
                self.stats.decisions = self.stats.decisions.saturating_add(1);
                self.enqueue(p, Reason::Decision);
                return Decision::Propagated;
            }
        }
        match self.pick_branch() {
            None => Decision::Sat,
            Some(lit) => {
                self.decisions.push(self.trail.len());
                self.stats.decisions = self.stats.decisions.saturating_add(1);
                self.enqueue(lit, Reason::Decision);
                Decision::Propagated
            }
        }
    }

    /// Drive the search to a terminal state under the current assumptions.
    /// `Ok(())` = SAT; `Err(Unsat(core))` = UNSAT with a sufficient subset of
    /// the assumptions (empty when unsat regardless of them);
    /// `Err(Exhausted)` = the conflict budget ran out (no answer — only
    /// possible when a budget is installed). Re-entrant: after
    /// [`Solver::block`] resets to level 0, calling it again continues the search.
    pub(crate) fn run(&mut self) -> Result<(), RunFail> {
        if !self.ok {
            return Err(RunFail::Unsat(Vec::new()));
        }
        loop {
            if let Some(cref) = self.propagate() {
                if self.current_level() == 0 {
                    self.ok = false;
                    return Err(RunFail::Unsat(Vec::new()));
                }
                // A budget of n admits exactly n analyzed conflicts; the n+1-th
                // aborts here, before analysis. Terminal level-0 UNSAT above is
                // checked first: a real answer always beats giving up.
                if let Some(budget) = &self.budget
                    && !budget.spend()
                {
                    return Err(RunFail::Exhausted);
                }
                let (learned, backjump) = self.analyze(cref);
                self.backtrack(backjump);
                self.learn(learned);
            } else {
                match self.decide() {
                    Decision::Propagated => {}
                    Decision::Sat => return Ok(()),
                    Decision::UnsatCore(core) => return Err(RunFail::Unsat(core)),
                }
            }
        }
    }

    /// Plain satisfiability (no assumptions): `true` if a model exists. Re-entrant
    /// for [`Models`] enumeration. Fails only when an installed [`Budget`] runs out.
    pub(crate) fn search(&mut self) -> Result<bool, BudgetExhausted> {
        match self.run() {
            Ok(()) => Ok(true),
            Err(RunFail::Unsat(_)) => Ok(false),
            Err(RunFail::Exhausted) => Err(BudgetExhausted),
        }
    }

    /// Snapshot the assignment as `var -> bool` (any still-unassigned variable,
    /// possible when it is unconstrained, defaults to false).
    pub(crate) fn model(&self) -> Vec<bool> {
        self.assign.iter().map(|a| a.unwrap_or(false)).collect()
    }

    /// Forbid the current `model`'s projection, then reset to level 0 so the next
    /// [`Solver::search`] finds a different model. Returns `false` if the
    /// projection is empty (there is only one model to report).
    pub(crate) fn block(&mut self, project: &[Var], model: &[bool]) -> bool {
        if project.is_empty() {
            return false;
        }
        let block: Vec<SatLit> = project
            .iter()
            .map(|&v| {
                if model[v as usize] {
                    SatLit::negative(v)
                } else {
                    SatLit::positive(v)
                }
            })
            .collect();
        self.backtrack(0);
        self.add_clause(&block);
        true
    }

    // -- the incremental (re-entrant) interface --

    /// Re-entrant assumption solve: rewind to level 0, install `assumptions`, and
    /// drive the search to a terminal state. Level-0 consequences (root units,
    /// learned clauses) persist across calls — that is the whole point: a sequence
    /// of related queries shares one clause database instead of re-solving from
    /// scratch. Same contract as [`Solver::run`].
    pub(crate) fn solve_with(&mut self, assumptions: &[SatLit]) -> Result<(), RunFail> {
        self.backtrack(0);
        self.assumptions.clear();
        self.assumptions.extend_from_slice(assumptions);
        self.run()
    }

    /// Attach a clause at level 0, rewinding first so the two-watched invariant
    /// holds (mirrors [`Solver::block`]). For clauses added between incremental
    /// queries, e.g. guarded blocking clauses.
    pub(crate) fn add_clause_root(&mut self, lits: &[SatLit]) {
        self.backtrack(0);
        self.add_clause(lits);
    }

    /// Grow the variable universe by one fresh (unconstrained, unassigned)
    /// variable and return it. Lets a caller mint session variables (e.g.
    /// blocking-clause guards) on demand instead of pre-declaring the lot —
    /// a variable that does not exist yet costs no decisions.
    pub(crate) fn add_var(&mut self) -> Var {
        let v = self.assign.len() as Var;
        self.assign.push(None);
        self.level.push(0);
        self.reason.push(Reason::Decision);
        self.activity.push(0.0);
        self.polarity.push(false);
        self.seen.push(false);
        self.watches.push(Vec::new());
        self.watches.push(Vec::new());
        self.order.push_var(&self.activity, v);
        v
    }

    /// The cumulative work counters (never reset).
    pub(crate) fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Switch the heuristics profile from the next solve on.
    pub(crate) fn set_config(&mut self, config: SolverConfig) {
        self.config = config;
    }

    /// Install (or remove) a shared conflict [`Budget`]. With `None` — the
    /// default — no solve on this solver can ever abort.
    pub(crate) fn set_budget(&mut self, budget: Option<Budget>) {
        self.budget = budget;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The work counters pin at `u64::MAX` instead of panicking (debug) or
    /// silently wrapping (release). With plain `+=` this test dies with an
    /// overflow panic on the very first decision.
    #[test]
    fn stats_saturate_at_u64_max() {
        // UNSAT over two vars: exercises every counter at least once
        // (a decision, propagations, a conflict, a learned clause).
        let a = SatLit::positive(0);
        let b = SatLit::positive(1);
        let mut cnf = Cnf::new(2);
        cnf.clauses.push(vec![a, b]);
        cnf.clauses.push(vec![a, b.negate()]);
        cnf.clauses.push(vec![a.negate(), b]);
        cnf.clauses.push(vec![a.negate(), b.negate()]);

        let mut s = Solver::new(&cnf);
        s.stats = Stats {
            decisions: u64::MAX,
            propagations: u64::MAX,
            conflicts: u64::MAX,
            learned_literals: u64::MAX,
        };
        assert!(s.run().is_err(), "the formula is UNSAT");
        let saturated = s.stats();
        assert_eq!(saturated.decisions, u64::MAX);
        assert_eq!(saturated.propagations, u64::MAX);
        assert_eq!(saturated.conflicts, u64::MAX);
        assert_eq!(saturated.learned_literals, u64::MAX);
    }
}
