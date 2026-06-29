//! The abstract syntax tree: the typed shape a `.vrf` source parses into.
//!
//! Everything here is zero-copy over the source `&str` (atoms/names borrow their
//! slices) and every node that can be pointed at in an error carries its
//! [`Span`] via [`Located`].

use alloc::vec::Vec;

use nom_locate::LocatedSpan;

/// Source code fragment with line and column tracking.
pub type Span<'a> = LocatedSpan<&'a str>;

/// Container for data associated with its source location.
#[derive(Debug, Clone, PartialEq)]
pub struct Located<'a, T> {
    /// The actual parsed data.
    pub data: T,
    /// The location in the source text (start of the construct).
    pub span: Span<'a>,
}

/// An atom is the triple `(subject, predicate, object?)` — the unit of identity —
/// optionally qualified by a domain (`physics.engine has fuel`).
/// `Creature_A has flying` and `Creature_A has swimming` are DIFFERENT atoms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atom<'a> {
    /// The domain this atom is qualified into, written as a `domain.` prefix on
    /// the subject (e.g. `physics` in `physics.engine has fuel`). `None` means the
    /// atom belongs to the current file's own declared domain (no prefix).
    pub domain: Option<&'a str>,
    /// The entity the claim is about, e.g. `Creature_A` or `Motor`.
    pub subject: &'a str,
    /// The relation or property asserted, e.g. `has` or `over_100`. `None` for a
    /// **bare proposition** — a single-word atom such as `db_ready`, introduced by
    /// a `VAR` port and used directly in bodies (`WHEN db_ready THEN …`). The
    /// compiler requires any bare-proposition atom to be a declared `VAR`.
    pub predicate: Option<&'a str>,
    /// Optional value the predicate relates the subject to, e.g. `flying`.
    /// `None` for two-word atoms such as `Motor over_100`. The object is part of
    /// identity: `has flying` and `has swimming` are different atoms. Always `None`
    /// when `predicate` is `None` (a bare proposition has neither).
    pub object: Option<&'a str>,
}

/// A literal is an atom, optionally negated (`NOT ...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal<'a> {
    /// `true` when written with a leading `NOT` (asserts the atom is FALSE).
    pub negated: bool,
    /// The underlying atom being asserted true or false.
    pub atom: Atom<'a>,
}

/// List constraint operators (body of a list-style `PREMISE`).
///
/// These are surface sugar; the compiler desugars each to `Impossible` clauses
/// (see `elenchus-compiler`). The meanings below are *what the author asserts*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOp {
    /// `EXCLUSIVE` — at most one of the listed atoms may be TRUE (mutual
    /// exclusion). For n > 2 this is pairwise, not "not all at once".
    Exclusive,
    /// `FORBIDS` — at most one may be TRUE; a synonym of [`ListOp::Exclusive`]
    /// (same pairwise expansion), kept as a separate word for readability.
    Forbids,
    /// `ONEOF` — exactly one is TRUE: at-most-one (pairwise) plus at-least-one.
    OneOf,
    /// `ATLEAST` — at least one of the listed atoms is TRUE.
    AtLeast,
}

/// How the literals in a `WHEN`/`THEN` group combine. A single-literal group is
/// always [`Conn::And`] (the connective is irrelevant with one literal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conn {
    /// Continuation lines used `AND` — all literals must hold.
    And,
    /// Continuation lines used `OR` — at least one literal must hold.
    Or,
}

/// A `FOR EACH` quantifier on a `PREMISE`/`RULE` header. It instantiates the
/// premise's whole body **once per element**, substituting the binder. The
/// quantifier lives in the *header* (not the body), so there is exactly one per
/// statement and the body grammar is untouched — a second `FOR EACH` is
/// structurally unrepresentable, which is what bounds the desugar to a linear
/// number of clauses (no domain products).
#[derive(Debug, Clone, PartialEq)]
pub enum Quant<'a> {
    /// `FOR EACH <binder> IN <set>` — range over the elements of a declared
    /// [`Statement::Set`]. The binder is the name the body's atoms refer to.
    InSet {
        /// The name bound inside the body (substituted per element).
        binder: Located<'a, &'a str>,
        /// The declared set this ranges over.
        set: Located<'a, &'a str>,
    },
    /// `FOR EACH <left> <predicate> <right>` — range over the declared `FACT`
    /// pairs of that relation (e.g. every `FACT a linked b`), binding `left` to a
    /// pair's subject and `right` to its object. This is the channel for
    /// multi-element constraints (graphs, dependencies): the pair is *pinned by
    /// data*, so the desugar is linear in the number of facts, never a product.
    Relation {
        /// Bound to each matching fact's subject.
        left: Located<'a, &'a str>,
        /// The relation predicate whose facts are ranged over.
        predicate: Located<'a, &'a str>,
        /// Bound to each matching fact's object.
        right: Located<'a, &'a str>,
    },
}

/// Which closure a `CLOSE` statement applies to a relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseKind {
    /// `TRANSITIVE` — add `a→c` whenever `a→b` and `b→c` hold (requires a DAG).
    Transitive,
}

/// The body of an `PREMISE` or `RULE`.
#[derive(Debug, Clone, PartialEq)]
pub enum Body<'a> {
    /// `EXCLUSIVE`/`FORBIDS`/`ONEOF`/`ATLEAST` over >= 2 atoms.
    List {
        /// Which list constraint this is.
        op: ListOp,
        /// The atoms it ranges over (the parser guarantees at least two).
        atoms: Vec<Located<'a, Atom<'a>>>,
    },
    /// `WHEN ... [AND|OR ...] THEN ... [AND|OR ...]` — antecedent + consequent.
    /// Within one group the continuation keyword is uniform (no mixing `AND`/`OR`).
    Impl {
        /// `WHEN`/`AND`/`OR` conditions.
        antecedent: Vec<Located<'a, Literal<'a>>>,
        /// How the antecedent literals combine.
        ante_conn: Conn,
        /// `THEN`/`AND`/`OR` results that follow when the antecedent holds.
        consequent: Vec<Located<'a, Literal<'a>>>,
        /// How the consequent literals combine.
        cons_conn: Conn,
    },
}

/// A top-level statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement<'a> {
    /// `DOMAIN <name>` — declare the domain this file's atoms belong to. Required
    /// once per file, as the first statement; it is the identity namespace into
    /// which bare atoms fall.
    Domain(Located<'a, &'a str>),
    /// `IMPORT "path" [AS <alias>]` — reuse another source (resolved by the
    /// compiler). The optional `alias` is the local name the imported domain is
    /// referenced by; without it, the imported file's own declared domain name is
    /// used.
    Import {
        /// The quoted source path.
        path: Located<'a, &'a str>,
        /// The local alias for the imported domain, if `AS <alias>` was given.
        alias: Option<Located<'a, &'a str>>,
    },
    /// `FACT <atom>` — a TRUE assertion.
    Fact(Located<'a, Atom<'a>>),
    /// `NOT <atom>` — a FALSE assertion.
    Negation(Located<'a, Atom<'a>>),
    /// `ASSUME [NOT] <atom>` — a *soft* (retractable) assertion. Same shape as a
    /// `FACT`/`NOT`, but it is a hypothesis, not a commitment: when the
    /// assumptions cannot all hold the solver names which to drop, and it never
    /// blames a `FACT`/`PREMISE`. The `Literal` carries the optional `NOT`.
    Assume(Located<'a, Literal<'a>>),
    /// `SET <name>` then one element identifier per line — declare a finite set
    /// to quantify a `PREMISE`/`RULE` over via `FOR EACH <binder> IN <name>`.
    Set {
        /// The set's name (referenced by `FOR EACH … IN <name>`).
        name: Located<'a, &'a str>,
        /// Its elements, one per line (at least one).
        elements: Vec<Located<'a, &'a str>>,
    },
    /// `CLOSE <relation> TRANSITIVE` — close a relation's `FACT` pairs under the
    /// given kind at compile time (a graph operation, no solver cost). A cycle is
    /// a compile error.
    Close {
        /// The relation predicate to close (e.g. `depends_on`).
        relation: Located<'a, &'a str>,
        /// Which closure to apply.
        kind: CloseKind,
    },
    /// `VAR <name> [DEFAULT true|false]` — declare an external boolean **port**: a
    /// single-word proposition whose truth is supplied from outside (CLI/API/data).
    /// `<name>` doubles as the proposition usable in bodies (`WHEN <name> THEN …`)
    /// and as the key external values bind to. With no supplied value it falls back
    /// to `default`, or stays UNKNOWN when there is none.
    Var {
        /// The port's name — both the bound proposition and the external key.
        name: Located<'a, &'a str>,
        /// The `DEFAULT true|false` fallback, if written.
        default: Option<bool>,
    },
    /// `PREMISE <name> [FOR EACH …]: ...` — a checked first principle, optionally
    /// quantified over a declared set.
    Premise {
        /// The premise's label (a per-source name, not a global identifier).
        name: Located<'a, &'a str>,
        /// An optional `FOR EACH … IN …` header quantifier (at most one).
        quant: Option<Quant<'a>>,
        /// The constraint itself: a list body or a `WHEN … THEN` implication.
        body: Body<'a>,
    },
    /// `RULE <name> [FOR EACH …]: ...` — a fact-producing inference rule.
    Rule {
        /// The rule's label.
        name: Located<'a, &'a str>,
        /// An optional `FOR EACH … IN …` header quantifier (at most one).
        quant: Option<Quant<'a>>,
        /// Always an implication body (the grammar forbids a list body here).
        body: Body<'a>,
    },
    /// `CHECK [subject] [BIDIRECTIONAL]` — a query.
    Check {
        /// Restrict the report to this subject; `None` checks everything.
        subject: Option<Located<'a, &'a str>>,
        /// `true` enables the backward (all-SAT) pass for UNDERDETERMINED.
        bidirectional: bool,
    },
}

/// A parsed program: a flat sequence of statements.
#[derive(Debug, Clone, PartialEq)]
pub struct Program<'a> {
    /// Top-level statements in source order. The list is flat: PREMISE/RULE bodies
    /// live inside their [`Statement`], not as separate entries.
    pub statements: Vec<Statement<'a>>,
}
