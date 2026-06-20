//! Human-facing syntax diagnostics: the owned, renderable result of a failed
//! parse.
//!
//! [`parse`](crate::parse) collects every error in one pass into [`Diagnostics`]
//! (it recovers after each broken statement). Rendering is ASCII-only and
//! deterministic so a dumb terminal shows it correctly and snapshots stay
//! stable. Each [`Diagnostic`] becomes a block: the line number, the original
//! line, a caret under the problem, the message, and — via
//! [`syntax_for`](crate::syntax::syntax_for) — the keyword's correct syntax and
//! a real example.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};

use crate::syntax::{TOP_LEVEL_FORMS, syntax_for};

/// One syntax error, fully owned (no borrow of the source) so it can flow into
/// `CompileError` and be rendered later with any error limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 1-based source line of the failure.
    pub(crate) line: usize,
    /// 1-based column (in characters) where the caret points.
    pub(crate) col: usize,
    /// How many caret characters to draw (at least one).
    pub(crate) width: usize,
    /// The specific parser message ("FACT expects an atom: …").
    pub(crate) message: String,
    /// The keyword this error is about, if one is named in the message — selects
    /// the syntax card.
    pub(crate) keyword: Option<&'static str>,
    /// `true` for a line that started no statement keyword at all: show the menu
    /// of valid top-level statements rather than a single card.
    pub(crate) general: bool,
    /// The verbatim offending source line (without its line ending).
    pub(crate) line_text: String,
}

/// Every syntax error from one parse, plus the source label for the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostics {
    /// The source label (file name) for the header; `None` until the compiler
    /// attaches it.
    pub(crate) file: Option<String>,
    /// The errors, in source order.
    pub(crate) errors: Vec<Diagnostic>,
}

impl Diagnostics {
    /// How many errors were found.
    pub fn len(&self) -> usize {
        self.errors.len()
    }

    /// Whether there are no errors (a `Diagnostics` is only constructed when the
    /// parse failed, so in practice this is always `false`).
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Attach (or replace) the source label shown in the header — used by the
    /// compiler, which knows the file name the parser does not.
    pub fn set_file(&mut self, file: &str) {
        self.file = Some(String::from(file));
    }

    /// Render every error, or the first `limit` of them.
    ///
    /// `None` (or `Some(0)`, or a limit ≥ the count) shows all; a smaller limit
    /// shows the first N blocks and a `(showing N of TOTAL)` footer so the input
    /// is not flooded.
    pub fn render(&self, limit: Option<usize>) -> String {
        let total = self.errors.len();
        let shown = match limit {
            Some(n) if n > 0 && n < total => n,
            _ => total,
        };

        let noun = if total == 1 { "error" } else { "errors" };
        let mut out = String::new();
        match &self.file {
            Some(f) => {
                let _ = write!(out, "RESULT: {total} syntax {noun} in {f}");
            }
            None => {
                let _ = write!(out, "RESULT: {total} syntax {noun}");
            }
        }

        for (i, d) in self.errors.iter().take(shown).enumerate() {
            out.push_str("\n\n");
            render_block(&mut out, i + 1, total, d);
        }

        if shown < total {
            let _ = write!(
                out,
                "\n\n(showing {shown} of {total} — pass --max-errors 0 for all)"
            );
        }
        out
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(None))
    }
}

/// The `   | ` gutter that prefixes the source line and the caret line.
const GUTTER: &str = "   | ";
/// Indentation under a `label : value` line, so continuation lines of a
/// multi-line value align with the value (3 + 7 + 3 = 13 columns).
const VALUE_INDENT: &str = "             ";

/// Append one error block (no trailing newline) to `out`.
fn render_block(out: &mut String, idx: usize, total: usize, d: &Diagnostic) {
    let _ = writeln!(out, "[{idx}/{total}] line {}, col {}", d.line, d.col);
    let _ = writeln!(out, "{GUTTER}{}", d.line_text);

    let pad = " ".repeat(d.col.saturating_sub(1));
    let carets = "^".repeat(d.width.max(1));
    let _ = writeln!(out, "{GUTTER}{pad}{carets}");

    label(out, "problem", &d.message);
    match d.keyword.and_then(syntax_for) {
        // A keyword is named in the message → show its card.
        Some(card) => {
            label(out, "syntax", card.form());
            label(out, "example", card.example());
        }
        // A line that starts no keyword at all → show the menu of statements.
        None if d.general => {
            out.push_str("   expected one of these statements:");
            for kw in TOP_LEVEL_FORMS {
                if let Some(c) = syntax_for(kw) {
                    let _ = write!(out, "\n       {}", c.form());
                }
            }
        }
        // Otherwise the message is self-explanatory; no card to add.
        None => {}
    }
    // Strip the trailing newline left by the last `writeln!` so blocks join
    // with a single blank line.
    if out.ends_with('\n') {
        out.pop();
    }
}

/// Write a `   label   : value` line; continuation lines of a multi-line value
/// are indented to align under the value.
fn label(out: &mut String, name: &str, value: &str) {
    let mut lines = value.split('\n');
    let first = lines.next().unwrap_or("");
    let _ = writeln!(out, "   {name:<7} : {first}");
    for line in lines {
        let _ = writeln!(out, "{VALUE_INDENT}{line}");
    }
}
