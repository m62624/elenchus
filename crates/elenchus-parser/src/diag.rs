//! Human-facing syntax diagnostics: the owned, renderable result of a failed
//! parse.
//!
//! [`parse`](crate::parse) collects every error in one pass into [`Diagnostics`]
//! (it recovers after each broken statement). Rendering **groups errors by
//! class** — the keyword they are about — so the correct syntax and a real
//! example are shown *once per class*, with every offending place listed beneath
//! it (line, caret, the specific problem). Two independent caps control the
//! volume: how many classes, and how many places per class. Output is ASCII-only
//! and deterministic so a dumb terminal shows it correctly and snapshots stay
//! stable.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};

use crate::syntax::{TOP_LEVEL_FORMS, syntax_for};

/// One syntax error, fully owned (no borrow of the source) so it can flow into
/// `CompileError` and be rendered later with any limit.
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
    /// the class and its syntax card.
    pub(crate) keyword: Option<&'static str>,
    /// `true` for a line that started no statement keyword at all: its class is
    /// the statement menu rather than a single card.
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

/// The class an error is grouped under: a specific keyword, the "not a
/// statement" menu, or self-explanatory leftovers with no card.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A keyword named in the message — shows that keyword's syntax card.
    Keyword(&'static str),
    /// A line that started no top-level keyword — shows the statement menu.
    Statement,
    /// A self-explanatory error with no card (e.g. "needs at least two atoms").
    Other,
}

impl Class {
    /// The class an error belongs to.
    fn of(d: &Diagnostic) -> Class {
        match (d.keyword, d.general) {
            (Some(kw), _) => Class::Keyword(kw),
            (None, true) => Class::Statement,
            (None, false) => Class::Other,
        }
    }

    /// The class label shown in the header and the "… and N more" footer.
    fn name(self) -> &'static str {
        match self {
            Class::Keyword(kw) => kw,
            Class::Statement => "statement",
            Class::Other => "other",
        }
    }
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

    /// Render the errors grouped by class.
    ///
    /// `max_classes` caps how many classes are shown; `max_per_class` caps how
    /// many places are listed within each class. `None` (or `Some(0)`, or a cap
    /// ≥ the count) means "all". A cap that hides some places adds a
    /// `… and N more <class> problems` line; a cap that hides some classes adds
    /// a `… and N more classes` footer.
    pub fn render(&self, max_classes: Option<usize>, max_per_class: Option<usize>) -> String {
        let groups = self.group();
        let total = self.errors.len();
        let total_classes = groups.len();
        let shown_classes = cap(max_classes, total_classes);

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

        for (class, items) in groups.iter().take(shown_classes) {
            out.push_str("\n\n");
            render_class(&mut out, *class, items, max_per_class);
        }

        if shown_classes < total_classes {
            let rest = total_classes - shown_classes;
            let plural = if rest == 1 { "class" } else { "classes" };
            let _ = write!(
                out,
                "\n\n... and {rest} more {plural} — pass --max-classes 0 for all"
            );
        }
        out
    }

    /// Group errors by [`Class`], preserving first-appearance order (so output
    /// is deterministic and follows the source top to bottom).
    fn group(&self) -> Vec<(Class, Vec<&Diagnostic>)> {
        let mut groups: Vec<(Class, Vec<&Diagnostic>)> = Vec::new();
        for d in &self.errors {
            let class = Class::of(d);
            match groups.iter_mut().find(|(c, _)| *c == class) {
                Some((_, items)) => items.push(d),
                None => groups.push((class, vec![d])),
            }
        }
        groups
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(None, None))
    }
}

/// Resolve an optional cap against a total: `None`/`Some(0)`/`≥ total` ⇒ all.
fn cap(limit: Option<usize>, total: usize) -> usize {
    match limit {
        Some(n) if n > 0 && n < total => n,
        _ => total,
    }
}

/// The `      | ` gutter that prefixes a place's source line and caret line.
const PLACE_GUTTER: &str = "      | ";

/// Append one class block (no trailing newline) to `out`.
fn render_class(
    out: &mut String,
    class: Class,
    items: &[&Diagnostic],
    max_per_class: Option<usize>,
) {
    let name = class.name();
    let n = items.len();
    let problems = if n == 1 { "problem" } else { "problems" };
    let _ = writeln!(out, "{name}  ({n} {problems})");

    // The correct-syntax reference, shown once for the whole class.
    match class {
        Class::Keyword(kw) => {
            if let Some(card) = syntax_for(kw) {
                label(out, "syntax", card.form());
                label(out, "example", card.example());
            }
        }
        Class::Statement => {
            out.push_str("  expected one of these statements:");
            for kw in TOP_LEVEL_FORMS {
                if let Some(card) = syntax_for(kw) {
                    let _ = write!(out, "\n      {}", card.form());
                }
            }
            out.push('\n');
        }
        // Self-explanatory: each place carries its own message, no shared card.
        Class::Other => {}
    }

    let shown = cap(max_per_class, n);
    for d in items.iter().take(shown) {
        render_place(out, d);
    }
    if shown < n {
        let rest = n - shown;
        let p = if rest == 1 { "problem" } else { "problems" };
        let _ = writeln!(out, "    ... and {rest} more {name} {p}");
    }

    // Strip the trailing newline so classes join with a single blank line.
    if out.ends_with('\n') {
        out.pop();
    }
}

/// Append one place: its location + message, the source line, and the caret.
fn render_place(out: &mut String, d: &Diagnostic) {
    let _ = writeln!(out, "    line {}, col {} - {}", d.line, d.col, d.message);
    let _ = writeln!(out, "{PLACE_GUTTER}{}", d.line_text);
    let pad = " ".repeat(d.col.saturating_sub(1));
    let carets = "^".repeat(d.width.max(1));
    let _ = writeln!(out, "{PLACE_GUTTER}{pad}{carets}");
}

/// Write a `  label   : value` line under a class header; continuation lines of
/// a multi-line value align under the value (2 + 7 + 3 = 12 columns).
fn label(out: &mut String, name: &str, value: &str) {
    const VALUE_INDENT: &str = "            ";
    let mut lines = value.split('\n');
    let first = lines.next().unwrap_or("");
    let _ = writeln!(out, "  {name:<7} : {first}");
    for line in lines {
        let _ = writeln!(out, "{VALUE_INDENT}{line}");
    }
}
