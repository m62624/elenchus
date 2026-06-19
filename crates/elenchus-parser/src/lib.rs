//! elenchus-parser — parses the English-like elenchus DSL into an AST.
//!
//! Style mirrors `vsm-parser`: zero-copy over `&str`, `nom` + `nom_locate`
//! for line/column tracking, and a human-friendly `^--- here` error display.
//! Syntax is line/keyword-oriented (not S-expressions) so small models cannot
//! trip on parentheses or indentation.
//!
//! Grammar (see docs/SPEC.md, "Grammar (EBNF)"):
//! - statements are newline-terminated; indentation is cosmetic, not significant;
//! - keywords are ALWAYS CAPS (ASCII); identifiers are content (case-sensitive,
//!   verbatim, any-script letters — e.g. `условие`, `名前`);
//! - block boundaries (PREMISE/RULE bodies) are found by keywords, never by indent.
#![no_std]
// Every public item is documented; CI (`clippy -D warnings`) keeps it that way.
#![warn(missing_docs)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_while},
    character::complete::{char, line_ending, satisfy, space0, space1},
    combinator::{eof, opt, recognize, value},
    multi::many0,
    sequence::{delimited, preceded, terminated},
};
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

// --- AST -------------------------------------------------------------------

/// An atom is the triple `(subject, predicate, object?)` — the unit of identity.
/// `Creature.A has flying` and `Creature.A has swimming` are DIFFERENT atoms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Atom<'a> {
    /// The entity the claim is about, e.g. `Creature.A` or `Motor`.
    pub subject: &'a str,
    /// The relation or property asserted, e.g. `has` or `over_100`.
    pub predicate: &'a str,
    /// Optional value the predicate relates the subject to, e.g. `flying`.
    /// `None` for two-word atoms such as `Motor over_100`. The object is part of
    /// identity: `has flying` and `has swimming` are different atoms.
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
    /// `IMPORT "path"` — reuse another source (resolved by the compiler).
    Import(Located<'a, &'a str>),
    /// `FACT <atom>` — a TRUE assertion.
    Fact(Located<'a, Atom<'a>>),
    /// `NOT <atom>` — a FALSE assertion.
    Negation(Located<'a, Atom<'a>>),
    /// `PREMISE <name>: ...` — a checked first principle.
    Premise {
        /// The premise's label (a per-source name, not a global identifier).
        name: Located<'a, &'a str>,
        /// The constraint itself: a list body or a `WHEN … THEN` implication.
        body: Body<'a>,
    },
    /// `RULE <name>: ...` — a fact-producing inference rule.
    Rule {
        /// The rule's label.
        name: Located<'a, &'a str>,
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

/// Reserved words — always CAPS, in full. An identifier may not equal any of these.
pub const RESERVED: &[&str] = &[
    "IMPORT",
    "FACT",
    "NOT",
    "PREMISE",
    "RULE",
    "CHECK",
    "BIDIRECTIONAL",
    "WHEN",
    "AND",
    "OR",
    "THEN",
    "EXCLUSIVE",
    "FORBIDS",
    "ONEOF",
    "ATLEAST",
];

/// Whether `word` is a reserved keyword.
pub fn is_reserved(word: &str) -> bool {
    RESERVED.contains(&word)
}

// --- Error -----------------------------------------------------------------

/// A friendly error structure that can be displayed to the user.
#[derive(Debug)]
pub struct ParseError<'a> {
    /// The original full input string (needed for context).
    pub source: &'a str,
    /// The span where the error occurred.
    pub span: Span<'a>,
    /// A description of the error.
    pub message: String,
}

impl<'a> fmt::Display for ParseError<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let line = self.span.location_line() as usize;
        let column = self.span.get_column();
        let full_line = self
            .source
            .lines()
            .nth(line.saturating_sub(1))
            .unwrap_or("");
        let indent = " ".repeat(if column > 0 { column - 1 } else { 0 });

        write!(
            f,
            "Syntax Error at line {}, col {}: {}\n  | {}\n  | {}^--- here",
            line, column, self.message, full_line, indent
        )
    }
}

// --- Parser primitives -----------------------------------------------------

/// Internal nom error carrying a human message and the precise failure span.
/// Recoverable failures are `nom::Err::Error`; once a statement has committed
/// (its leading keyword is recognized), failures are promoted to
/// `nom::Err::Failure` so the error points at the *real* problem with a specific
/// message instead of backtracking to a generic "expected a statement".
#[derive(Debug, Clone)]
struct Problem<'a> {
    input: Span<'a>,
    message: String,
}

impl<'a> nom::error::ParseError<Span<'a>> for Problem<'a> {
    fn from_error_kind(input: Span<'a>, _: nom::error::ErrorKind) -> Self {
        Problem {
            input,
            message: String::from("unexpected token"),
        }
    }
    fn append(_: Span<'a>, _: nom::error::ErrorKind, other: Self) -> Self {
        other
    }
}

/// nom result specialized to our [`Problem`] error and located `Span` input.
type PResult<'a, T> = IResult<Span<'a>, T, Problem<'a>>;

/// Turn a recoverable `Error` into a committed `Failure` at `at` with `msg`.
/// Already-`Failure` (a deeper, more specific message) and `Ok` pass through.
fn promote<'a, T>(r: PResult<'a, T>, at: Span<'a>, msg: &str) -> PResult<'a, T> {
    match r {
        Err(nom::Err::Error(_)) => Err(nom::Err::Failure(Problem {
            input: at,
            message: String::from(msg),
        })),
        other => other,
    }
}

/// A plain recoverable `Error` at `input` (lets an enclosing `alt` backtrack).
fn perr<'a, T>(input: Span<'a>) -> PResult<'a, T> {
    Err(nom::Err::Error(Problem {
        input,
        message: String::from("unexpected token"),
    }))
}

/// Characters allowed *after* the first in an identifier. Letters and digits of
/// *any* script are accepted (Unicode `is_alphanumeric`), so `условие` or `名前`
/// are valid; `_` joins multi-word names and `.` makes dotted subjects like
/// `Creature.A` a single token. Punctuation and other symbols are rejected.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// A bare identifier (does not reject reserved words). The first character must
/// be a letter of any script (`is_alphabetic`) — never a digit, `_`, `.`, or
/// punctuation — so identifiers stay distinct from numbers and operators.
fn raw_identifier<'a>(input: Span<'a>) -> PResult<'a, Span<'a>> {
    recognize((satisfy(|c| c.is_alphabetic()), take_while(is_ident_char))).parse(input)
}

/// An identifier that is not a reserved keyword.
fn identifier<'a>(input: Span<'a>) -> PResult<'a, Located<'a, &'a str>> {
    let start = input;
    let (rest, sp) = raw_identifier(input)?;
    if is_reserved(sp.fragment()) {
        return perr(start);
    }
    Ok((
        rest,
        Located {
            data: *sp.fragment(),
            span: start,
        },
    ))
}

/// A line comment `// ... ` up to (but not including) the line ending.
fn comment<'a>(input: Span<'a>) -> PResult<'a, Span<'a>> {
    recognize((tag("//"), take_while(|c| c != '\n' && c != '\r'))).parse(input)
}

/// End of a statement line: trailing spaces, an optional trailing comment,
/// then a line ending or EOF.
fn eol<'a>(input: Span<'a>) -> PResult<'a, ()> {
    value((), (space0, opt(comment), alt((line_ending, eof)))).parse(input)
}

/// A blank line or a full-line comment (must consume a line ending to progress).
fn noise_line<'a>(input: Span<'a>) -> PResult<'a, ()> {
    value((), (space0, opt(comment), line_ending)).parse(input)
}

/// Skip any number of blank / comment lines between statements.
fn skip_noise<'a>(input: Span<'a>) -> PResult<'a, ()> {
    value((), many0(noise_line)).parse(input)
}

// --- Atoms & literals ------------------------------------------------------

/// `<subject> <predicate> [<object>]` — two or three space-separated identifiers.
fn atom<'a>(input: Span<'a>) -> PResult<'a, Located<'a, Atom<'a>>> {
    let start = input;
    let (input, subject) = identifier(input)?;
    let (input, _) = space1(input)?;
    let (input, predicate) = identifier(input)?;
    let (input, object) = opt(preceded(space1, identifier)).parse(input)?;
    Ok((
        input,
        Located {
            data: Atom {
                subject: subject.data,
                predicate: predicate.data,
                object: object.map(|o| o.data),
            },
            span: start,
        },
    ))
}

/// An optionally `NOT`-prefixed [`atom`] — a literal inside a `WHEN`/`THEN` body.
fn literal<'a>(input: Span<'a>) -> PResult<'a, Located<'a, Literal<'a>>> {
    let start = input;
    let (input, neg) = opt(terminated(tag("NOT"), space1)).parse(input)?;
    let (input, a) = atom(input)?;
    Ok((
        input,
        Located {
            data: Literal {
                negated: neg.is_some(),
                atom: a.data,
            },
            span: start,
        },
    ))
}

/// An atom on its own (possibly indented) line: used inside list bodies.
fn atom_line<'a>(input: Span<'a>) -> PResult<'a, Located<'a, Atom<'a>>> {
    let (input, _) = space0(input)?;
    let (input, a) = atom(input)?;
    let (input, _) = eol(input)?;
    Ok((input, a))
}

// --- Bodies ----------------------------------------------------------------

/// One of the list-constraint keywords (`EXCLUSIVE`/`FORBIDS`/`ONEOF`/`ATLEAST`).
fn list_op<'a>(input: Span<'a>) -> PResult<'a, ListOp> {
    alt((
        value(ListOp::Exclusive, tag("EXCLUSIVE")),
        value(ListOp::Forbids, tag("FORBIDS")),
        value(ListOp::OneOf, tag("ONEOF")),
        value(ListOp::AtLeast, tag("ATLEAST")),
    ))
    .parse(input)
}

/// A list-constraint body: a list operator on its own line, then one atom per
/// line (at least two).
///
/// Commit strategy: the leading `list_op` failing stays a recoverable `Error`
/// so the `PREMISE` `alt` can fall through and try [`impl_body`] instead. *Once*
/// the operator matched we are committed, so every subsequent failure is
/// [`promote`]d to a `Failure` with a specific message — no backtracking to a
/// generic "expected a statement".
fn list_body<'a>(input: Span<'a>) -> PResult<'a, Body<'a>> {
    let (input, _) = space0(input)?;
    // list_op failing stays Error so the PREMISE alt can try impl_body.
    let (input, op) = list_op(input)?;
    // Past here we are committed to a list body.
    let (input, _) = promote(
        eol(input),
        input,
        "expected a newline after the list operator",
    )?;
    let at = input;
    let (input, first) = promote(
        atom_line(input),
        at,
        "a list premise needs at least two atoms",
    )?;
    let at = input;
    let (input, second) = promote(
        atom_line(input),
        at,
        "a list premise needs at least two atoms",
    )?;
    let (input, rest) = many0(atom_line).parse(input)?;

    let mut atoms = vec![first, second];
    atoms.extend(rest);
    Ok((input, Body::List { op, atoms }))
}

/// A continuation `AND <literal>` / `OR <literal>` line inside a `WHEN`/`THEN`
/// block. Returns `(Conn, literal)`. A line that is neither `AND` nor `OR` yields
/// a recoverable `Error` so `many0` stops cleanly (e.g. at `THEN`/EOF).
fn cont_line<'a>(input: Span<'a>) -> PResult<'a, (Conn, Located<'a, Literal<'a>>)> {
    let (input, _) = space0(input)?;
    // Not an AND/OR line → Error so many0 stops cleanly.
    let (input, conn) =
        alt((value(Conn::And, tag("AND")), value(Conn::Or, tag("OR")))).parse(input)?;
    let (input, _) = space1(input)?;
    let at = input;
    let (input, lit) = promote(
        literal(input),
        at,
        "AND/OR expects a literal: [NOT] <Subject> <predicate> [<object>]",
    )?;
    let (input, _) = promote(
        eol(input),
        input,
        "unexpected text after the AND/OR literal",
    )?;
    Ok((input, (conn, lit)))
}

/// Reduce a group's continuation lines to a single [`Conn`], rejecting a mix of
/// `AND` and `OR` in one group (point the error at the first line that switches).
fn group_conn<'a>(conts: &[(Conn, Located<'a, Literal<'a>>)]) -> Result<Conn, Span<'a>> {
    let mut seen: Option<Conn> = None;
    for (conn, lit) in conts {
        match seen {
            None => seen = Some(*conn),
            Some(s) if s != *conn => return Err(lit.span),
            _ => {}
        }
    }
    Ok(seen.unwrap_or(Conn::And))
}

/// Fail with a committed message at `at` (for in-body semantic checks).
fn fail_at<'a, T>(at: Span<'a>, msg: &str) -> PResult<'a, T> {
    Err(nom::Err::Failure(Problem {
        input: at,
        message: String::from(msg),
    }))
}

/// An implication body: `WHEN <lit> [AND <lit>]* THEN <lit> [AND <lit>]*`.
///
/// Like [`list_body`], a missing leading `WHEN` stays a recoverable `Error` (so
/// the `PREMISE` `alt` can try a list body); after `WHEN` matches we are committed
/// and use [`promote`] for precise errors. Antecedent and consequent each start
/// with one mandatory literal followed by zero or more `AND` lines.
fn impl_body<'a>(input: Span<'a>) -> PResult<'a, Body<'a>> {
    let (input, _) = space0(input)?;
    // No WHEN → Error so the PREMISE alt can fall through to a list body.
    let (input, _) = (tag("WHEN"), space1).parse(input)?;
    // Committed to an implication body now.
    let at = input;
    let (input, when) = promote(
        literal(input),
        at,
        "WHEN expects a literal: [NOT] <Subject> <predicate> [<object>]",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the WHEN literal")?;
    let (input, ante_rest) = many0(cont_line).parse(input)?;
    let ante_conn = match group_conn(&ante_rest) {
        Ok(c) => c,
        Err(span) => {
            return fail_at(
                span,
                "don't mix AND and OR in one WHEN group — split it into separate premises",
            );
        }
    };

    let (input, _) = space0(input)?;
    let at = input;
    let (input, _) = promote(
        tag("THEN").parse(input),
        at,
        "expected THEN to complete the WHEN ... THEN implication",
    )?;
    let at = input;
    let (input, then) = promote(
        preceded(space1, literal).parse(input),
        at,
        "THEN expects a literal: [NOT] <Subject> <predicate> [<object>]",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the THEN literal")?;
    let (input, cons_rest) = many0(cont_line).parse(input)?;
    let cons_conn = match group_conn(&cons_rest) {
        Ok(c) => c,
        Err(span) => {
            return fail_at(
                span,
                "don't mix AND and OR in one THEN group — split it into separate premises",
            );
        }
    };

    let mut antecedent = vec![when];
    antecedent.extend(ante_rest.into_iter().map(|(_, l)| l));
    let mut consequent = vec![then];
    consequent.extend(cons_rest.into_iter().map(|(_, l)| l));
    Ok((
        input,
        Body::Impl {
            antecedent,
            ante_conn,
            consequent,
            cons_conn,
        },
    ))
}

// --- Statements ------------------------------------------------------------

/// `IMPORT "<path>"` — a quoted path on one line.
fn stmt_import<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag("IMPORT"), space1).parse(input)?;
    let start = input;
    let (input, path) = promote(
        delimited(char('"'), take_while(|c| c != '"' && c != '\n'), char('"')).parse(input),
        start,
        "IMPORT expects a quoted path, e.g. IMPORT \"physics.vrf\"",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the IMPORT path")?;
    Ok((
        input,
        Statement::Import(Located {
            data: *path.fragment(),
            span: start,
        }),
    ))
}

/// `FACT <atom>` — a TRUE assertion.
fn stmt_fact<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag("FACT"), space1).parse(input)?;
    let at = input;
    let (input, a) = promote(
        atom(input),
        at,
        "FACT expects an atom: <Subject> <predicate> [<object>]",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the FACT atom")?;
    Ok((input, Statement::Fact(a)))
}

/// `NOT <atom>` — a FALSE assertion. Tried last among statements so a body-level
/// `NOT` literal is never mistaken for a top-level negation.
fn stmt_negation<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag("NOT"), space1).parse(input)?;
    let at = input;
    let (input, a) = promote(
        atom(input),
        at,
        "NOT expects an atom: <Subject> <predicate> [<object>]",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the NOT atom")?;
    Ok((input, Statement::Negation(a)))
}

/// `CHECK [<subject>] [BIDIRECTIONAL]` — both modifiers optional.
fn stmt_check<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = tag("CHECK").parse(input)?;
    let (input, subject) = opt(preceded(space1, identifier)).parse(input)?;
    let (input, bidir) = opt(preceded(space1, tag("BIDIRECTIONAL"))).parse(input)?;
    let (input, _) = eol(input)?;
    Ok((
        input,
        Statement::Check {
            subject,
            bidirectional: bidir.is_some(),
        },
    ))
}

/// `PREMISE <name>: <body>` where the body is a list or an implication.
fn stmt_premise<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag("PREMISE"), space1).parse(input)?;
    // Committed to a premise now.
    let at = input;
    let (input, name) = promote(
        identifier(input),
        at,
        "expected a premise name (a lowercase identifier)",
    )?;
    let (input, _) = space0(input)?;
    let (input, _) = promote(
        char(':').parse(input),
        input,
        "expected ':' after the premise name",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after 'PREMISE <name>:'")?;
    let at = input;
    let (input, body) = promote(
        alt((list_body, impl_body)).parse(input),
        at,
        "a premise body must be a list (EXCLUSIVE/FORBIDS/ONEOF/ATLEAST) or WHEN ... THEN",
    )?;
    Ok((input, Statement::Premise { name, body }))
}

/// `RULE <name>: <implication>` — like a premise but the body must be `WHEN … THEN`.
fn stmt_rule<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag("RULE"), space1).parse(input)?;
    let at = input;
    let (input, name) = promote(
        identifier(input),
        at,
        "expected a rule name (a lowercase identifier)",
    )?;
    let (input, _) = space0(input)?;
    let (input, _) = promote(
        char(':').parse(input),
        input,
        "expected ':' after the rule name",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after 'RULE <name>:'")?;
    let at = input;
    let (input, body) = promote(impl_body(input), at, "a rule body must be WHEN ... THEN")?;
    Ok((input, Statement::Rule { name, body }))
}

/// One top-level statement. Order matters: each branch commits on its keyword,
/// and `stmt_negation` comes last so a leading `NOT` is only read as a top-level
/// negation when nothing else matched.
fn statement<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    // Leading indentation is cosmetic everywhere, including on the top-level
    // keyword line — so a whole program may be written indented (e.g. inside a
    // host's here-doc) and parse identically.
    let (input, _) = space0(input)?;
    alt((
        stmt_import,
        stmt_fact,
        stmt_premise,
        stmt_rule,
        stmt_check,
        stmt_negation,
    ))
    .parse(input)
}

/// The whole program: leading noise, then statements each followed by noise
/// (blank/comment lines). Stops at the first byte that is not a valid statement;
/// [`parse`] then checks the tail is empty and otherwise reports an error there.
fn program<'a>(input: Span<'a>) -> PResult<'a, Vec<Statement<'a>>> {
    let (input, _) = skip_noise(input)?;
    many0(terminated(statement, skip_noise)).parse(input)
}

/// Parse a full `.vrf` source into a [`Program`].
///
/// On error, returns a [`ParseError`] whose `Display` shows the offending line
/// with a `^--- here` caret, mirroring vsm-parser.
pub fn parse(src: &str) -> Result<Program<'_>, ParseError<'_>> {
    let input = Span::new(src);
    match program(input) {
        Ok((rest, statements)) => {
            if !trailing_is_empty(rest.fragment()) {
                return Err(ParseError {
                    source: src,
                    span: rest,
                    message: String::from(
                        "expected a statement (IMPORT/FACT/NOT/PREMISE/RULE/CHECK)",
                    ),
                });
            }
            Ok(Program { statements })
        }
        Err(nom::Err::Error(e)) | Err(nom::Err::Failure(e)) => Err(ParseError {
            source: src,
            span: e.input,
            message: e.message,
        }),
        Err(nom::Err::Incomplete(_)) => Err(ParseError {
            source: src,
            span: input,
            message: String::from("incomplete input"),
        }),
    }
}

/// True if the unparsed tail is only whitespace and trailing comments.
fn trailing_is_empty(tail: &str) -> bool {
    for raw in tail.lines() {
        let t = raw.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn prog(src: &str) -> Program<'_> {
        parse(src).expect("should parse")
    }

    /// `(subject, predicate, object?)` of one atom, borrowed from the source.
    type AtomShape<'a> = (&'a str, &'a str, Option<&'a str>);
    /// A list premise flattened to `(operator, its atoms)`.
    type ListShape<'a> = (ListOp, Vec<AtomShape<'a>>);

    /// Atom data flattened to owned tuples — span-independent, for structural
    /// comparison (spans differ by offset, which is exactly what "cosmetic" means).
    fn atom_shapes<'a>(p: &Program<'a>) -> Vec<ListShape<'a>> {
        p.statements
            .iter()
            .filter_map(|s| match s {
                Statement::Premise {
                    body: Body::List { op, atoms },
                    ..
                } => Some((
                    *op,
                    atoms
                        .iter()
                        .map(|a| (a.data.subject, a.data.predicate, a.data.object))
                        .collect(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_fact_and_negation() {
        let p = prog("FACT Creature.A has flying\nNOT Creature.A has cold_blood\n");
        assert_eq!(p.statements.len(), 2);
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.subject, "Creature.A");
                assert_eq!(a.data.predicate, "has");
                assert_eq!(a.data.object, Some("flying"));
            }
            other => panic!("expected fact, got {:?}", other),
        }
        match &p.statements[1] {
            Statement::Negation(a) => {
                assert_eq!(a.data.object, Some("cold_blood"));
            }
            other => panic!("expected negation, got {:?}", other),
        }
    }

    #[test]
    fn fact_without_object() {
        let p = prog("FACT Motor over_100\n");
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.subject, "Motor");
                assert_eq!(a.data.predicate, "over_100");
                assert_eq!(a.data.object, None);
            }
            other => panic!("expected fact, got {:?}", other),
        }
    }

    #[test]
    fn parses_import() {
        let p = prog("IMPORT \"physics.vrf\"\n");
        match &p.statements[0] {
            Statement::Import(path) => assert_eq!(path.data, "physics.vrf"),
            other => panic!("expected import, got {:?}", other),
        }
    }

    #[test]
    fn parses_exclusive_premise() {
        let src = "PREMISE fly_xor_swim:\n    EXCLUSIVE\n        Creature.A has flying\n        Creature.A has swimming\n";
        let p = prog(src);
        match &p.statements[0] {
            Statement::Premise { name, body } => {
                assert_eq!(name.data, "fly_xor_swim");
                match body {
                    Body::List { op, atoms } => {
                        assert_eq!(*op, ListOp::Exclusive);
                        assert_eq!(atoms.len(), 2);
                        assert_eq!(atoms[1].data.object, Some("swimming"));
                    }
                    other => panic!("expected list body, got {:?}", other),
                }
            }
            other => panic!("expected premise, got {:?}", other),
        }
    }

    #[test]
    fn parses_implication_premise_with_and() {
        let src = "PREMISE wings_need_bone:\n    WHEN Creature.A has flying\n    THEN Creature.A has wing\n    AND  Creature.A has bone\n";
        let p = prog(src);
        match &p.statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                        ..
                    },
                ..
            } => {
                assert_eq!(antecedent.len(), 1);
                assert_eq!(antecedent[0].data.atom.object, Some("flying"));
                assert_eq!(consequent.len(), 2);
                assert_eq!(consequent[0].data.atom.object, Some("wing"));
                assert_eq!(consequent[1].data.atom.object, Some("bone"));
            }
            other => panic!("expected impl premise, got {:?}", other),
        }
    }

    #[test]
    fn antecedent_and_goes_before_then() {
        let src = "PREMISE deploy:\n    WHEN s tested\n    AND s reviewed\n    THEN s can_deploy\n";
        let p = prog(src);
        match &p.statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        antecedent,
                        consequent,
                        ..
                    },
                ..
            } => {
                assert_eq!(antecedent.len(), 2);
                assert_eq!(consequent.len(), 1);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn when_or_sets_disjunctive_antecedent() {
        let src = "PREMISE p:\n    WHEN x a\n    OR x b\n    THEN x c\n";
        match &prog(src).statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        antecedent,
                        ante_conn,
                        consequent,
                        cons_conn,
                    },
                ..
            } => {
                assert_eq!(antecedent.len(), 2);
                assert_eq!(*ante_conn, Conn::Or);
                assert_eq!(consequent.len(), 1);
                assert_eq!(*cons_conn, Conn::And); // single consequent → AND
            }
            other => panic!("expected impl premise, got {:?}", other),
        }
    }

    #[test]
    fn then_or_sets_disjunctive_consequent() {
        let src = "PREMISE p:\n    WHEN x a\n    THEN x b\n    OR x c\n";
        match &prog(src).statements[0] {
            Statement::Premise {
                body:
                    Body::Impl {
                        consequent,
                        cons_conn,
                        ..
                    },
                ..
            } => {
                assert_eq!(consequent.len(), 2);
                assert_eq!(*cons_conn, Conn::Or);
            }
            other => panic!("expected impl premise, got {:?}", other),
        }
    }

    #[test]
    fn mixing_and_or_in_one_group_is_an_error() {
        assert!(
            parse("PREMISE p:\n    WHEN x a\n    AND x b\n    OR x c\n    THEN x d\n").is_err()
        );
        assert!(
            parse("PREMISE p:\n    WHEN x a\n    THEN x b\n    AND x c\n    OR x d\n").is_err()
        );
    }

    #[test]
    fn or_is_a_reserved_word() {
        assert!(parse("FACT OR has x\n").is_err());
    }

    #[test]
    fn parses_negated_literal_in_rule() {
        let src = "RULE pick_slow:\n    WHEN NOT Motor over_100\n    THEN Motor uses slow_path\n";
        let p = prog(src);
        match &p.statements[0] {
            Statement::Rule {
                body: Body::Impl { antecedent, .. },
                ..
            } => {
                assert!(antecedent[0].data.negated);
                assert_eq!(antecedent[0].data.atom.predicate, "over_100");
            }
            other => panic!("expected rule, got {:?}", other),
        }
    }

    #[test]
    fn parses_check_variants() {
        let p = prog("CHECK Creature.A BIDIRECTIONAL\n");
        match &p.statements[0] {
            Statement::Check {
                subject,
                bidirectional,
            } => {
                assert_eq!(subject.as_ref().unwrap().data, "Creature.A");
                assert!(bidirectional);
            }
            other => panic!("expected check, got {:?}", other),
        }

        let p = prog("CHECK\n");
        match &p.statements[0] {
            Statement::Check {
                subject,
                bidirectional,
            } => {
                assert!(subject.is_none());
                assert!(!bidirectional);
            }
            other => panic!("expected check, got {:?}", other),
        }
    }

    #[test]
    fn comments_and_blanks_are_ignored() {
        let src = "// header\n\nFACT a b   // trailing comment\n\n// tail\n";
        let p = prog(src);
        assert_eq!(p.statements.len(), 1);
    }

    #[test]
    fn indentation_is_cosmetic() {
        let flat = "PREMISE x:\nEXCLUSIVE\na b\na c\n";
        let indented = "PREMISE x:\n        EXCLUSIVE\n  a b\n            a c\n";
        // Spans differ by offset (cosmetic); the parsed structure must be identical.
        assert_eq!(atom_shapes(&prog(flat)), atom_shapes(&prog(indented)));
    }

    #[test]
    fn top_level_statements_may_be_indented() {
        // Leading indentation on the FACT/PREMISE/CHECK lines themselves is also
        // cosmetic (so a whole program can be pasted indented inside a here-doc).
        let flat = "FACT x a\nNOT x b\nCHECK x\n";
        let indented = "    FACT x a\n        NOT x b\n    CHECK x\n";
        assert_eq!(atom_shapes(&prog(flat)), atom_shapes(&prog(indented)));
        assert_eq!(prog(indented).statements.len(), 3);
    }

    #[test]
    fn full_creature_example_parses() {
        let src = include_str!("../../../docs/examples/creature.vrf");
        let p = prog(src);
        // 2 FACT + 3 PREMISE + 1 RULE + 1 CHECK = 7
        assert_eq!(p.statements.len(), 7);
    }

    #[test]
    fn import_demo_example_parses() {
        let src = include_str!("../../../docs/examples/import-demo.vrf");
        let p = prog(src);
        assert!(matches!(p.statements[0], Statement::Import(_)));
    }

    #[test]
    fn unicode_identifiers_any_script() {
        // Cyrillic subject/predicate/object, mixed with `_` and digits (not first).
        let p = prog("FACT кот пушистый2\nNOT собака has крылья\n");
        match &p.statements[0] {
            Statement::Fact(a) => {
                assert_eq!(a.data.subject, "кот");
                assert_eq!(a.data.predicate, "пушистый2");
                assert_eq!(a.data.object, None);
            }
            other => panic!("expected fact, got {:?}", other),
        }
        match &p.statements[1] {
            Statement::Negation(a) => {
                assert_eq!(a.data.subject, "собака");
                assert_eq!(a.data.object, Some("крылья"));
            }
            other => panic!("expected negation, got {:?}", other),
        }
    }

    #[test]
    fn unicode_premise_name_and_body() {
        let src = "PREMISE правило_лая:\n    WHEN собака has хвост\n    THEN собака умеет_лаять\n";
        match &prog(src).statements[0] {
            Statement::Premise { name, body } => {
                assert_eq!(name.data, "правило_лая");
                match body {
                    Body::Impl {
                        antecedent,
                        consequent,
                        ..
                    } => {
                        assert_eq!(antecedent[0].data.atom.subject, "собака");
                        assert_eq!(consequent[0].data.atom.subject, "собака");
                        assert_eq!(consequent[0].data.atom.predicate, "умеет_лаять");
                    }
                    other => panic!("expected impl body, got {:?}", other),
                }
            }
            other => panic!("expected premise, got {:?}", other),
        }
    }

    #[test]
    fn identifier_cannot_start_with_digit() {
        // `2cats` is not a valid subject — first char must be a letter.
        assert!(parse("FACT 2cats has fur\n").is_err());
    }

    #[test]
    fn punctuation_is_rejected_in_identifier() {
        // `!` and other symbols are not identifier characters.
        assert!(parse("FACT cat! has fur\n").is_err());
    }

    #[test]
    fn reserved_word_cannot_be_identifier() {
        // `WHEN` as a subject is illegal.
        assert!(parse("FACT WHEN has x\n").is_err());
    }

    #[test]
    fn pretty_error_points_at_offending_line() {
        let src = "FACT a b\n!garbage here\nFACT c d\n";
        let err = parse(src).expect_err("should fail");
        let shown = format!("{}", err);
        assert!(shown.contains("Syntax Error"));
        assert!(shown.contains("line 2"));
        assert!(shown.contains("!garbage here"));
        assert!(shown.contains("^--- here"));
    }

    #[test]
    fn crlf_line_endings() {
        let p = prog("FACT a b\r\nCHECK a\r\n");
        assert_eq!(p.statements.len(), 2);
    }

    #[test]
    fn tabs_as_indentation() {
        let p = prog("PREMISE e:\n\tEXCLUSIVE\n\t\tx a\n\t\tx b\n");
        assert!(matches!(
            p.statements[0],
            Statement::Premise {
                body: Body::List {
                    op: ListOp::Exclusive,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn parses_all_list_ops() {
        for (kw, want) in [
            ("EXCLUSIVE", ListOp::Exclusive),
            ("FORBIDS", ListOp::Forbids),
            ("ONEOF", ListOp::OneOf),
            ("ATLEAST", ListOp::AtLeast),
        ] {
            let src = alloc::format!("PREMISE a:\n    {kw}\n        x a\n        x b\n");
            match &prog(&src).statements[0] {
                Statement::Premise {
                    body: Body::List { op, .. },
                    ..
                } => assert_eq!(*op, want),
                other => panic!("{kw}: unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn check_bidirectional_without_subject() {
        match &prog("CHECK BIDIRECTIONAL\n").statements[0] {
            Statement::Check {
                subject,
                bidirectional,
            } => {
                assert!(subject.is_none());
                assert!(bidirectional);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_and_comment_only_input_yield_no_statements() {
        assert_eq!(prog("").statements.len(), 0);
        assert_eq!(prog("// just a comment\n\n   \n").statements.len(), 0);
    }

    #[test]
    fn negation_with_object() {
        match &prog("NOT Creature.A has wing\n").statements[0] {
            Statement::Negation(a) => {
                assert_eq!(a.data.subject, "Creature.A");
                assert_eq!(a.data.object, Some("wing"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn negated_consequent_then_not() {
        let src = "PREMISE a:\n    WHEN x on\n    THEN NOT x off\n";
        match &prog(src).statements[0] {
            Statement::Premise {
                body: Body::Impl { consequent, .. },
                ..
            } => {
                assert!(consequent[0].data.negated);
                assert_eq!(consequent[0].data.atom.predicate, "off");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn multiple_imports_then_facts() {
        let p = prog("IMPORT \"a.vrf\"\nIMPORT \"b.vrf\"\nFACT x y\n");
        assert!(matches!(p.statements[0], Statement::Import(_)));
        assert!(matches!(p.statements[1], Statement::Import(_)));
        assert!(matches!(p.statements[2], Statement::Fact(_)));
    }

    #[test]
    fn trailing_comment_without_final_newline() {
        let p = prog("FACT a b\n// trailing, no newline");
        assert_eq!(p.statements.len(), 1);
    }
}
