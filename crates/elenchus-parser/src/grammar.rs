//! The nom grammar and the recovering top-level driver.
//!
//! The per-statement parsers (`atom`, `literal`, `stmt_*`, `impl_body`,
//! `list_body`) are ordinary `nom` combinators over a located [`Span`]; once a
//! statement commits on its leading keyword, failures are [`promote`]d to a
//! `Failure` carrying a precise message ([`Problem`]). The driver [`parse`] does
//! not stop at the first failure: it records a [`Diagnostic`], resynchronises to
//! the next top-level keyword line, and keeps going, so one run reports *every*
//! syntax error.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use nom::{
    IResult, Parser,
    branch::alt,
    bytes::complete::{tag, take_while},
    character::complete::{char, line_ending, satisfy, space0, space1},
    combinator::{eof, opt, recognize, value},
    multi::many0,
    sequence::{delimited, preceded, terminated},
};

use crate::ast::{
    Atom, Body, CloseKind, Conn, ListOp, Literal, Located, Program, Quant, Span, Statement,
};
use crate::diag::{Diagnostic, Diagnostics};
use crate::keywords::{is_reserved, is_top_level, keyword_in, kw};

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
/// are valid; `_` joins multi-word names. `.` is **not** an identifier character:
/// it is the domain separator (`physics.engine`), so use `_` for compound
/// subjects (`Creature_A`). Punctuation and other symbols are rejected.
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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

/// `[<domain>.]<subject> [<predicate> [<object>]]` — an atom, optionally qualified
/// by a `domain.` prefix on the subject, then one to three space-separated
/// identifiers. The domain is recognised only when an identifier is immediately
/// followed by `.` (no space), so a bare `engine has_fuel` keeps `engine` as the
/// subject. A lone identifier (`db_ready`) is a **bare proposition** — predicate
/// and object both `None`; the compiler requires it to be a declared `VAR`.
fn atom<'a>(input: Span<'a>) -> PResult<'a, Located<'a, Atom<'a>>> {
    let start = input;
    let (input, domain) = opt(terminated(identifier, char('.'))).parse(input)?;
    let (input, subject) = identifier(input)?;
    let (input, predicate) = opt(preceded(space1, identifier)).parse(input)?;
    // An object can only follow a predicate; a bare proposition has neither.
    let (input, object) = match predicate {
        Some(_) => opt(preceded(space1, identifier)).parse(input)?,
        None => (input, None),
    };
    Ok((
        input,
        Located {
            data: Atom {
                domain: domain.map(|d| d.data),
                subject: subject.data,
                predicate: predicate.map(|p| p.data),
                object: object.map(|o| o.data),
            },
            span: start,
        },
    ))
}

/// An optionally `NOT`-prefixed [`atom`] — a literal inside a `WHEN`/`THEN` body.
fn literal<'a>(input: Span<'a>) -> PResult<'a, Located<'a, Literal<'a>>> {
    let start = input;
    let (input, neg) = opt(terminated(tag(kw::NOT), space1)).parse(input)?;
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

/// A boolean value word `true` or `false` — the value of a `VAR` `DEFAULT` (and,
/// later, a `PROVIDE`). Parsed through the identifier tokenizer and matched
/// exactly, so `true`/`false` are not reserved and stay usable as ordinary atom
/// words; only in value position do they read as booleans.
fn bool_lit<'a>(input: Span<'a>) -> PResult<'a, bool> {
    let (rest, sp) = raw_identifier(input)?;
    match *sp.fragment() {
        "true" => Ok((rest, true)),
        "false" => Ok((rest, false)),
        _ => perr(input),
    }
}

/// An atom on its own (possibly indented) line: used inside list bodies.
fn atom_line<'a>(input: Span<'a>) -> PResult<'a, Located<'a, Atom<'a>>> {
    let (input, _) = space0(input)?;
    let (input, a) = atom(input)?;
    let (input, _) = eol(input)?;
    Ok((input, a))
}

/// A single identifier on its own (possibly indented) line: a `SET` element. A
/// reserved word or a non-identifier line yields a recoverable `Error` so `many0`
/// stops cleanly at the next statement.
fn element_line<'a>(input: Span<'a>) -> PResult<'a, Located<'a, &'a str>> {
    let (input, _) = space0(input)?;
    let (input, id) = identifier(input)?;
    let (input, _) = eol(input)?;
    Ok((input, id))
}

// --- Bodies ----------------------------------------------------------------

/// One of the list-constraint keywords (`EXCLUSIVE`/`FORBIDS`/`ONEOF`/`ATLEAST`).
fn list_op<'a>(input: Span<'a>) -> PResult<'a, ListOp> {
    alt((
        value(ListOp::Exclusive, tag(kw::EXCLUSIVE)),
        value(ListOp::Forbids, tag(kw::FORBIDS)),
        value(ListOp::OneOf, tag(kw::ONEOF)),
        value(ListOp::AtLeast, tag(kw::ATLEAST)),
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
/// The diagnostic for a list body with fewer than the required two atoms — one
/// spelling, used for both the missing-first and missing-second slot.
const LIST_NEEDS_TWO_ATOMS: &str = "a list premise needs at least two atoms";

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
    let (input, first) = promote(atom_line(input), at, LIST_NEEDS_TWO_ATOMS)?;
    let at = input;
    let (input, second) = promote(atom_line(input), at, LIST_NEEDS_TWO_ATOMS)?;
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
        alt((value(Conn::And, tag(kw::AND)), value(Conn::Or, tag(kw::OR)))).parse(input)?;
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
    let (input, _) = (tag(kw::WHEN), space1).parse(input)?;
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
        tag(kw::THEN).parse(input),
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

/// `IMPORT "<path>" [AS <alias>]` — a quoted path, optionally bound to a local
/// domain alias, on one line.
fn stmt_import<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::IMPORT), space1).parse(input)?;
    let start = input;
    let (input, path) = promote(
        delimited(char('"'), take_while(|c| c != '"' && c != '\n'), char('"')).parse(input),
        start,
        "IMPORT expects a quoted path, e.g. IMPORT \"physics.vrf\"",
    )?;
    // Optional `AS <alias>`: a local name for the imported domain.
    let (input, alias) = opt(preceded((space1, tag(kw::AS), space1), identifier)).parse(input)?;
    let (input, _) = promote(
        eol(input),
        input,
        "unexpected text after the IMPORT path (did you mean AS <alias>?)",
    )?;
    Ok((
        input,
        Statement::Import {
            path: Located {
                data: *path.fragment(),
                span: start,
            },
            alias,
        },
    ))
}

/// `DOMAIN <name>` — declare this file's domain on one line.
fn stmt_domain<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::DOMAIN), space1).parse(input)?;
    let at = input;
    let (input, name) = promote(
        identifier(input),
        at,
        "DOMAIN expects a name (a lowercase identifier), e.g. DOMAIN physics",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the DOMAIN name")?;
    Ok((input, Statement::Domain(name)))
}

/// `FACT <atom>` — a TRUE assertion.
fn stmt_fact<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::FACT), space1).parse(input)?;
    let at = input;
    let (input, a) = promote(
        atom(input),
        at,
        "FACT expects an atom: <Subject> <predicate> [<object>]",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the FACT atom")?;
    Ok((input, Statement::Fact(a)))
}

/// `ASSUME [NOT] <atom>` — a soft (retractable) assertion. Accepts a leading
/// `NOT` (like a `WHEN`/`THEN` literal), so `ASSUME NOT x a` is FALSE-by-default.
fn stmt_assume<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::ASSUME), space1).parse(input)?;
    let at = input;
    let (input, lit) = promote(
        literal(input),
        at,
        "ASSUME expects an atom: [NOT] <Subject> <predicate> [<object>]",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the ASSUME atom")?;
    Ok((input, Statement::Assume(lit)))
}

/// `NOT <atom>` — a FALSE assertion. Tried last among statements so a body-level
/// `NOT` literal is never mistaken for a top-level negation.
fn stmt_negation<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::NOT), space1).parse(input)?;
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
    let (input, _) = tag(kw::CHECK).parse(input)?;
    let (input, subject) = opt(preceded(space1, identifier)).parse(input)?;
    let (input, bidir) = opt(preceded(space1, tag(kw::BIDIRECTIONAL))).parse(input)?;
    let (input, _) = eol(input)?;
    Ok((
        input,
        Statement::Check {
            subject,
            bidirectional: bidir.is_some(),
        },
    ))
}

/// `SET <name>` then one identifier per line (at least one) — declare a finite
/// set to quantify a `PREMISE`/`RULE` over via `FOR EACH <binder> IN <name>`.
fn stmt_set<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::SET), space1).parse(input)?;
    let at = input;
    let (input, name) = promote(
        identifier(input),
        at,
        "SET expects a name (a lowercase identifier), e.g. SET tasks",
    )?;
    let (input, _) = promote(eol(input), input, "unexpected text after the SET name")?;
    let at = input;
    let (input, first) = promote(
        element_line(input),
        at,
        "a SET needs at least one element — one identifier per line",
    )?;
    let (input, rest) = many0(element_line).parse(input)?;
    let mut elements = vec![first];
    elements.extend(rest);
    Ok((input, Statement::Set { name, elements }))
}

/// `VAR <name> [DEFAULT true|false]` — declare an external boolean port on one
/// line. The optional `DEFAULT` gives the fallback when no value is supplied.
fn stmt_var<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::VAR), space1).parse(input)?;
    let at = input;
    let (input, name) = promote(
        identifier(input),
        at,
        "VAR expects a name (a lowercase identifier), e.g. VAR db_ready",
    )?;
    // Optional `DEFAULT true|false`. Once DEFAULT matches, a bad value commits.
    let (input, has_default) = opt(preceded(space1, tag(kw::DEFAULT))).parse(input)?;
    let (input, default) = if has_default.is_some() {
        let (input, _) = promote(
            space1(input),
            input,
            "DEFAULT expects a value: DEFAULT true|false",
        )?;
        let at = input;
        let (input, v) = promote(bool_lit(input), at, "DEFAULT expects true or false")?;
        (input, Some(v))
    } else {
        (input, None)
    };
    let (input, _) = promote(
        eol(input),
        input,
        "unexpected text after the VAR declaration",
    )?;
    Ok((input, Statement::Var { name, default }))
}

/// `PROVIDE [<domain>.]<port|atom>: true|false` — bind an external value on one
/// line. The target reuses the [`atom`] grammar, so it accepts a lone port
/// (`PROVIDE db_ready: true`), a multi-word atom (`PROVIDE engine has_fuel:
/// true`), and a `domain.` prefix (`PROVIDE self.has_vision: true`).
fn stmt_provide<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::PROVIDE), space1).parse(input)?;
    let at = input;
    let (input, atom) = promote(
        atom(input),
        at,
        "PROVIDE expects a port or atom, e.g. PROVIDE db_ready: true",
    )?;
    let (input, _) = space0(input)?;
    let (input, _) = promote(
        char(':').parse(input),
        input,
        "PROVIDE expects ':' then a value: PROVIDE <port>: true|false",
    )?;
    let (input, _) = space0(input)?;
    let at = input;
    let (input, value) = promote(bool_lit(input), at, "PROVIDE expects true or false")?;
    let (input, _) = promote(eol(input), input, "unexpected text after the PROVIDE value")?;
    Ok((input, Statement::Provide { atom, value }))
}

/// `CLOSE <relation> TRANSITIVE` — close a relation's FACT pairs at compile time.
fn stmt_close<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::CLOSE), space1).parse(input)?;
    let at = input;
    let (input, relation) = promote(
        identifier(input),
        at,
        "CLOSE expects a relation name, e.g. CLOSE depends_on TRANSITIVE",
    )?;
    let (input, _) = promote(
        (space1, tag(kw::TRANSITIVE)).parse(input),
        input,
        "CLOSE expects a closure kind: CLOSE <relation> TRANSITIVE",
    )?;
    let (input, _) = promote(
        eol(input),
        input,
        "unexpected text after 'CLOSE … TRANSITIVE'",
    )?;
    Ok((
        input,
        Statement::Close {
            relation,
            kind: CloseKind::Transitive,
        },
    ))
}

/// The optional quantifier tail on a `PREMISE`/`RULE` header (between the name
/// and the `:`). One of two forms:
///   `FOR EACH <binder> IN <set>`         — over a declared SET, or
///   `FOR EACH <left> <relation> <right>` — over the declared FACT pairs.
/// There is exactly one quantifier and no production for a second, so nesting is
/// unrepresentable.
fn for_each<'a>(input: Span<'a>) -> PResult<'a, Quant<'a>> {
    // No FOR → recoverable Error so `opt` yields None and the `:` is parsed next.
    let (input, _) = tag(kw::FOR).parse(input)?;
    // Committed: FOR can only begin a quantifier here.
    let at = input;
    let (input, _) = promote(
        (space1, tag(kw::EACH), space1).parse(input),
        at,
        "FOR must be followed by EACH: FOR EACH <binder> IN <set>  (or  FOR EACH <a> <relation> <b>)",
    )?;
    let at = input;
    let (input, first) = promote(
        identifier(input),
        at,
        "expected a binder name after FOR EACH",
    )?;
    let (input, _) = promote(
        space1(input),
        input,
        "expected `IN <set>` or a relation `<rel> <binder>` after the FOR EACH binder",
    )?;
    // `IN <set>` → set quantifier; otherwise `<predicate> <right>` → relation.
    let after_in: PResult<'a, (Span<'a>, Span<'a>)> = (tag(kw::IN), space1).parse(input);
    if let Ok((rest, _)) = after_in {
        let at = rest;
        let (rest, set) = promote(identifier(rest), at, "expected a set name after IN")?;
        return Ok((rest, Quant::InSet { binder: first, set }));
    }
    let at = input;
    let (input, predicate) = promote(
        identifier(input),
        at,
        "expected a relation name (FOR EACH <a> <relation> <b>)",
    )?;
    let (input, _) = promote(
        space1(input),
        input,
        "expected the second binder after the relation (FOR EACH <a> <relation> <b>)",
    )?;
    let at = input;
    let (input, right) = promote(
        identifier(input),
        at,
        "expected the second binder (FOR EACH <a> <relation> <b>)",
    )?;
    Ok((
        input,
        Quant::Relation {
            left: first,
            predicate,
            right,
        },
    ))
}

/// `PREMISE <name> [FOR EACH …]: <body>` where the body is a list or an implication.
/// Parse the `<name> [FOR EACH …]:` header shared by PREMISE and RULE, after the
/// keyword has been consumed. The structure — the name, the single optional
/// quantifier, the colon, and the end of line — lives here once; each keyword
/// passes its own three diagnostics so the wording stays specific.
fn named_header<'a>(
    input: Span<'a>,
    name_msg: &'static str,
    colon_msg: &'static str,
    tail_msg: &'static str,
) -> PResult<'a, (Located<'a, &'a str>, Option<Quant<'a>>)> {
    let at = input;
    let (input, name) = promote(identifier(input), at, name_msg)?;
    let (input, quant) = opt(preceded(space1, for_each)).parse(input)?;
    let (input, _) = space0(input)?;
    let (input, _) = promote(char(':').parse(input), input, colon_msg)?;
    let (input, _) = promote(eol(input), input, tail_msg)?;
    Ok((input, (name, quant)))
}

fn stmt_premise<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::PREMISE), space1).parse(input)?;
    // Committed to a premise now.
    let (input, (name, quant)) = named_header(
        input,
        "expected a premise name (a lowercase identifier)",
        "expected ':' after the premise name",
        "unexpected text after 'PREMISE <name>:'",
    )?;
    let at = input;
    let (input, body) = promote(
        alt((list_body, impl_body)).parse(input),
        at,
        "a premise body must be a list (EXCLUSIVE/FORBIDS/ONEOF/ATLEAST) or WHEN ... THEN",
    )?;
    Ok((input, Statement::Premise { name, quant, body }))
}

/// `RULE <name> [FOR EACH …]: <implication>` — like a premise but the body must
/// be `WHEN … THEN`.
fn stmt_rule<'a>(input: Span<'a>) -> PResult<'a, Statement<'a>> {
    let (input, _) = (tag(kw::RULE), space1).parse(input)?;
    let (input, (name, quant)) = named_header(
        input,
        "expected a rule name (a lowercase identifier)",
        "expected ':' after the rule name",
        "unexpected text after 'RULE <name>:'",
    )?;
    let at = input;
    let (input, body) = promote(impl_body(input), at, "a rule body must be WHEN ... THEN")?;
    Ok((input, Statement::Rule { name, quant, body }))
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
        stmt_domain,
        stmt_import,
        stmt_set,
        stmt_close,
        stmt_var,
        stmt_provide,
        stmt_fact,
        stmt_assume,
        stmt_premise,
        stmt_rule,
        stmt_check,
        stmt_negation,
    ))
    .parse(input)
}

// --- Recovering driver -----------------------------------------------------

/// Message for a line that begins with no known top-level keyword. The list of
/// statements is built from [`keywords::top_level_menu`], so it always names
/// exactly the top-level keywords that exist — no hand-kept copy to drift.
fn not_a_statement() -> String {
    alloc::format!(
        "expected a statement — a line must start with {}",
        crate::keywords::top_level_menu()
    )
}

/// Parse a full `.vrf` source into a [`Program`], collecting *every* syntax error.
///
/// On the first failing statement the parser does not give up: it records a
/// [`Diagnostic`], resynchronises to the next top-level keyword line, and
/// continues. A clean parse returns the [`Program`]; otherwise it returns all
/// errors as [`Diagnostics`], whose `Display` renders each as a caret block with
/// the keyword's correct syntax.
pub fn parse(src: &str) -> Result<Program<'_>, Diagnostics> {
    let mut input = Span::new(src);
    let mut statements = Vec::new();
    let mut errors: Vec<Diagnostic> = Vec::new();

    loop {
        if let Ok((rest, _)) = skip_noise(input) {
            input = rest;
        }
        // Only a whitespace / trailing-comment tail remains — a clean end, not a
        // statement (the `r#"..."#` fixtures end with an indented closing line).
        if at_end(input.fragment()) {
            break;
        }
        match statement(input) {
            Ok((rest, stmt)) => {
                statements.push(stmt);
                input = rest;
            }
            // Committed: a keyword matched but the rest is wrong → precise message.
            Err(nom::Err::Failure(p)) => {
                errors.push(make_diag(src, p.input, p.message, false));
                input = resync(p.input);
            }
            // Recoverable: this line started no statement keyword at all.
            Err(nom::Err::Error(p)) => {
                errors.push(make_diag(src, p.input, not_a_statement(), true));
                input = resync(p.input);
            }
            // `complete` combinators never return Incomplete; stop defensively.
            Err(nom::Err::Incomplete(_)) => break,
        }
    }

    if errors.is_empty() {
        Ok(Program { statements })
    } else {
        Err(Diagnostics { file: None, errors })
    }
}

/// Whether only an ignorable tail remains: pure whitespace, or a trailing
/// `//` comment with no following newline. `skip_noise` has already consumed any
/// full blank/comment lines, so anything else is a real statement to parse.
fn at_end(frag: &str) -> bool {
    let t = frag.trim_start();
    t.is_empty() || (t.starts_with("//") && !t.contains('\n'))
}

/// Build one [`Diagnostic`] from the failure span. `general` marks a
/// not-tied-to-a-keyword error (shows the menu of top-level forms instead of a
/// single syntax card).
fn make_diag(src: &str, at: Span<'_>, message: String, general: bool) -> Diagnostic {
    let line = at.location_line() as usize;
    let col = at.get_column();
    let line_text = src.lines().nth(line.saturating_sub(1)).unwrap_or("");
    // The card follows the *message* (which names the construct in question),
    // not the line the parser stalled on — a stall often lands on the *next*
    // line (e.g. "expected THEN …" points at the CHECK after a bodyless WHEN),
    // whose leading word would be a misleading card.
    let keyword = if general { None } else { keyword_in(&message) };
    Diagnostic {
        line,
        col,
        width: caret_width(line_text, col),
        message,
        keyword,
        general,
        line_text: line_text.to_string(),
    }
}

/// Caret length: from `col` to the last non-whitespace character of the line (at
/// least one), so the underline covers the offending token / trailing text.
fn caret_width(line_text: &str, col: usize) -> usize {
    let start = col.saturating_sub(1);
    let trimmed_len = line_text.trim_end().chars().count();
    trimmed_len.saturating_sub(start).max(1)
}

/// Resynchronise after an error: skip the rest of the offending line, then any
/// lines until one begins (after cosmetic indentation) with a top-level keyword,
/// or EOF. This keeps a broken PREMISE body from cascading into spurious errors.
fn resync(at: Span<'_>) -> Span<'_> {
    let mut input = consume_line(at);
    loop {
        if input.fragment().is_empty() || starts_top_level(input) {
            return input;
        }
        input = consume_line(input);
    }
}

/// Consume through the next line ending (or to EOF if none remains). The
/// `complete` combinators never fail here, so the `unwrap_or` is unreachable.
fn consume_line(input: Span<'_>) -> Span<'_> {
    let parsed: PResult<'_, Span<'_>> =
        recognize((take_while(|c| c != '\n' && c != '\r'), opt(line_ending))).parse(input);
    parsed.map(|(rest, _)| rest).unwrap_or(input)
}

/// Whether `input` begins (after cosmetic indentation) with a top-level keyword.
fn starts_top_level(input: Span<'_>) -> bool {
    let after = match space0::<_, Problem<'_>>(input) {
        Ok((rest, _)) => rest,
        Err(_) => return false,
    };
    let word: String = after
        .fragment()
        .chars()
        .take_while(|c| c.is_ascii_uppercase())
        .collect();
    is_top_level(&word)
}
