//! The human-readable report rendering (the `Display for Report` path).
use super::json::status_name;
use super::{Report, Status, TraceReason, TraceStep};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use elenchus_compiler::{Origin, PlaceholderStatus, Value, kw};

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(status_name(*self))
    }
}

/// Format provenance as `name (KIND)  [source:line]` for the human report.
pub(crate) fn premise_tag(o: &Origin) -> String {
    let name = o.premise.as_deref().unwrap_or("-");
    alloc::format!("{} ({})  [{}:{}]", name, o.kind, o.source, o.line)
}

/// One derivation-trace line for the human report.
pub(crate) fn trace_line(step: &TraceStep) -> String {
    let v = match step.value {
        Value::True => "TRUE",
        Value::False => "FALSE",
    };
    match &step.reason {
        TraceReason::Asserted(o) => {
            alloc::format!(
                "{} = {}   [{} {}:{}]",
                step.atom,
                v,
                o.kind,
                o.source,
                o.line
            )
        }
        TraceReason::Derived { origin, from } => alloc::format!(
            "{} = {}   from {} ({})  [{}:{}]  <= {}",
            step.atom,
            v,
            origin.premise.as_deref().unwrap_or("-"),
            origin.kind,
            origin.source,
            origin.line,
            from.join(", ")
        ),
    }
}

/// Indentation levels for the human report. This module is the **single** place
/// leading whitespace is defined: every line is emitted through
/// [`ReportWriter::line`] with one of these as the `indent` argument, so no
/// format string ever carries leading spaces. To restyle the report, change a
/// number here — not spaces scattered across `write!` calls.
mod indent {
    /// `RESULT:` / `SUMMARY:` / `EXIT_CODE:` — flush left.
    pub const ROOT: usize = 0;
    /// A section header: `CONFLICT` / `WARNING` / `CORE` / `RETRACT` / `DERIVED`
    /// / `HINT` / `UNDERDETERMINED`.
    pub const SECTION: usize = 2;
    /// A line belonging to a section (conflict atoms, `blocked by:`, an `ASSUME`).
    pub const ITEM: usize = 6;
    /// A line nested under an item (a `why:` trace step, a `CORE` member).
    pub const NESTED: usize = 8;
}

/// The human report's one output primitive. It owns the indentation rule so
/// callers pass a semantic [`indent`] level and the text — never raw spaces.
struct ReportWriter<'a, 'b> {
    f: &'a mut fmt::Formatter<'b>,
}

impl ReportWriter<'_, '_> {
    /// Write `indent` leading spaces, the formatted text, then a newline.
    fn line(&mut self, indent: usize, args: fmt::Arguments<'_>) -> fmt::Result {
        write!(self.f, "{:width$}{}", "", args, width = indent)?;
        self.f.write_str("\n")
    }

    /// Like [`line`](Self::line) but without the trailing newline — for the final
    /// `EXIT_CODE` line, so the report ends exactly as it always has.
    fn tail(&mut self, indent: usize, args: fmt::Arguments<'_>) -> fmt::Result {
        write!(self.f, "{:width$}{}", "", args, width = indent)
    }
}

/// `emit!(out, LEVEL, "fmt", args…)` — one indented report line. A thin wrapper
/// over [`ReportWriter::line`] so call sites read `emit!(out, SECTION, …)` with
/// the indent as an explicit parameter and zero leading spaces in the string.
macro_rules! emit {
    ($out:expr, $indent:expr, $($arg:tt)*) => {
        $out.line($indent, format_args!($($arg)*))
    };
}

impl Report {
    /// Render the full human report. `show_placeholders` toggles the PLACEHOLDERS
    /// section; the `Display` impl passes `true`, the CLI `--hide-params` flag
    /// passes `false` to print only the verdict (the JSON form always keeps it).
    fn render(&self, f: &mut fmt::Formatter<'_>, show_placeholders: bool) -> fmt::Result {
        use indent::{ITEM, NESTED, ROOT, SECTION};
        let mut out = ReportWriter { f };

        emit!(out, ROOT, "RESULT: {}", self.status)?;

        // A pure assumption clash: lead with the one action a small model needs
        // and suppress the raw conflict / CORE pools (they would only echo the
        // ASSUME clause). The verdict is still CONFLICT (exit code 2).
        if !self.retract.is_empty() {
            // Spell out what is wrong and why — this report is the debugger a
            // small model reads. The commitments are sound; the hypotheses are
            // the only dial to turn, so say so explicitly before listing them.
            emit!(out, SECTION, "RETRACT  your FACTs and PREMISEs are fine.")?;
            emit!(
                out,
                ITEM,
                "But these ASSUME guesses cannot all be true together."
            )?;
            emit!(out, ITEM, "Remove or flip ONE of them, then check again:")?;
            for it in &self.retract {
                emit!(
                    out,
                    ITEM,
                    "ASSUME {}   [{}:{}]",
                    it.label,
                    it.origin.source,
                    it.origin.line
                )?;
            }
        } else {
            for c in &self.conflicts {
                emit!(out, SECTION, "CONFLICT  {}", premise_tag(&c.origin))?;
                for a in &c.atoms {
                    emit!(out, ITEM, "{}", a)?;
                }
                if !c.trace.is_empty() {
                    emit!(out, ITEM, "why:")?;
                    for step in &c.trace {
                        emit!(out, NESTED, "{}", trace_line(step))?;
                    }
                }
            }
            if !self.unsat_core.is_empty() {
                emit!(
                    out,
                    SECTION,
                    "CORE  smallest jointly-unsatisfiable set ({}):",
                    self.unsat_core.len()
                )?;
                for it in &self.unsat_core {
                    let name = if it.label.is_empty() { "-" } else { &it.label };
                    emit!(
                        out,
                        NESTED,
                        "{} ({})  [{}:{}]",
                        name,
                        it.origin.kind,
                        it.origin.source,
                        it.origin.line
                    )?;
                }
            }
        }

        // Many warnings often share one root cause (e.g. the same missing FACT),
        // so the `fix:` line is deduped in the human report — each distinct fix
        // prints once — to keep a wall of warnings readable. The full per-warning
        // hint is still in the JSON for tools that select/filter programmatically.
        let mut shown_fixes: Vec<&str> = Vec::new();
        for w in &self.warnings {
            emit!(out, SECTION, "WARNING   {}", premise_tag(&w.origin))?;
            emit!(out, ITEM, "blocked by: {}", w.blocked_by.join(", "))?;
            if let Some(hint) = &w.hint
                && !shown_fixes.contains(&hint.as_str())
            {
                shown_fixes.push(hint);
                emit!(out, ITEM, "fix: {hint}")?;
            }
        }
        if let Some(atom) = &self.underdetermined {
            emit!(out, SECTION, "UNDERDETERMINED  an alternative model exists")?;
            emit!(out, ITEM, "pin it down: add  FACT {atom}  or  NOT {atom}")?;
        }
        for d in &self.derived {
            let v = match d.value {
                Value::True => "TRUE",
                Value::False => "FALSE",
            };
            emit!(
                out,
                SECTION,
                "DERIVED   {} = {}   from {}",
                d.atom,
                v,
                premise_tag(&d.origin)
            )?;
        }
        for h in &self.hints {
            emit!(
                out,
                SECTION,
                "HINT      possible typo — '{}' and '{}' look like the same atom ({})",
                h.a,
                h.b,
                h.reason
            )?;
        }
        for o in &self.orphans {
            // Reconstruct the surface line; `kind` already carries the polarity
            // except for `ASSUME NOT`, where the value supplies it.
            let surface = if o.origin.kind == kw::ASSUME && matches!(o.value, Value::False) {
                alloc::format!("{} {} {}", kw::ASSUME, kw::NOT, o.atom)
            } else {
                alloc::format!("{} {}", o.origin.kind, o.atom)
            };
            emit!(
                out,
                SECTION,
                "ORPHAN    {} — not used by any premise or rule (no effect on the result)",
                surface
            )?;
        }
        for u in &self.unused_imports {
            let via = match &u.alias {
                Some(a) => alloc::format!("{} AS {}", u.domain, a),
                None => u.domain.clone(),
            };
            emit!(
                out,
                SECTION,
                "UNUSED IMPORT  {} — imported in {}:{} but never referenced (no effect on the result)",
                via,
                u.file,
                u.line
            )?;
        }
        // The PLACEHOLDERS section: every declared VAR port, how it resolved. A
        // debugging aid for whoever wires the values; suppressed by `--hide-params`.
        if show_placeholders {
            for p in &self.placeholders {
                match p.status {
                    PlaceholderStatus::Supplied => emit!(
                        out,
                        SECTION,
                        "PARAM     {} = {}   (supplied{})",
                        p.key,
                        bool_word(p.value),
                        p.origin
                            .as_deref()
                            .map(|o| alloc::format!(": {o}"))
                            .unwrap_or_default()
                    )?,
                    PlaceholderStatus::DefaultUsed => emit!(
                        out,
                        SECTION,
                        "PARAM     {} = {}   (DEFAULT)",
                        p.key,
                        bool_word(p.value)
                    )?,
                    PlaceholderStatus::Unset => emit!(
                        out,
                        SECTION,
                        "PARAM     {} = UNKNOWN   (no value supplied, no DEFAULT)",
                        p.key
                    )?,
                }
            }
        }

        let underdetermined = usize::from(self.status == Status::Underdetermined);
        emit!(
            out,
            ROOT,
            "SUMMARY: {} conflicts, {} underdetermined, {} warnings, {} derived",
            self.conflicts.len(),
            underdetermined,
            self.warnings.len(),
            self.derived.len()
        )?;
        out.tail(ROOT, format_args!("EXIT_CODE: {}", self.exit_code()))
    }

    /// The full human report as an owned string. `show_placeholders = false`
    /// (the CLI `--hide-params` flag) prints only the verdict, exactly as before
    /// the ports feature. The JSON form (`to_json`) always keeps the section.
    pub fn render_human(&self, show_placeholders: bool) -> String {
        alloc::format!(
            "{}",
            HumanReport {
                report: self,
                show_placeholders
            }
        )
    }
}

/// A `Display` adapter that renders a [`Report`] with a chosen `show_placeholders`
/// flag (the plain `Display` impl always shows them).
struct HumanReport<'a> {
    report: &'a Report,
    show_placeholders: bool,
}

impl fmt::Display for HumanReport<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.report.render(f, self.show_placeholders)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.render(f, true)
    }
}

/// `true`/`false` for a resolved port value (a supplied/default port is always
/// `Some`); an unset port renders `UNKNOWN` at its own call site.
pub(crate) fn bool_word(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "true",
        Some(false) => "false",
        None => "UNKNOWN",
    }
}
