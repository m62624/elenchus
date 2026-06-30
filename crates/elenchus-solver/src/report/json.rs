//! JSON serialization of a [`Report`] (stable, machine-readable output).
use super::{Report, Status, TraceReason, TraceStep};
use alloc::string::String;
use elenchus_compiler::{Origin, PlaceholderStatus, Value};

impl Report {
    /// CLI-style exit code: 0 = consistent, 1 = underdetermined/warnings, 2 = conflicts.
    pub fn exit_code(&self) -> i32 {
        match self.status {
            Status::Conflict => 2,
            Status::Underdetermined | Status::Warning => 1,
            Status::Consistent => 0,
        }
    }

    /// Render the report as a single-line JSON object (for tooling / MCP).
    /// Hand-written so the crate stays dependency-free and `no_std`.
    pub fn to_json(&self) -> String {
        use core::fmt::Write as _;
        let mut s = String::new();
        let _ = write!(s, "{{\"status\":");
        json_str(status_name(self.status), &mut s);
        let _ = write!(s, ",\"exit_code\":{}", self.exit_code());

        s.push_str(",\"conflicts\":[");
        for (i, c) in self.conflicts.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&c.origin, &mut s);
            s.push_str(",\"atoms\":");
            json_array(&c.atoms, &mut s);
            s.push_str(",\"trace\":[");
            for (j, step) in c.trace.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                json_trace_step(step, &mut s);
            }
            s.push_str("]}");
        }
        s.push_str("],\"warnings\":[");
        for (i, w) in self.warnings.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&w.origin, &mut s);
            s.push_str(",\"blocked_by\":");
            json_array(&w.blocked_by, &mut s);
            s.push_str(",\"hint\":");
            match &w.hint {
                Some(h) => json_str(h, &mut s),
                None => s.push_str("null"),
            }
            s.push('}');
        }
        s.push_str("],\"derived\":[");
        for (i, d) in self.derived.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('{');
            json_origin_fields(&d.origin, &mut s);
            s.push_str(",\"atom\":");
            json_str(&d.atom, &mut s);
            let _ = write!(s, ",\"value\":{}", matches!(d.value, Value::True));
            s.push('}');
        }
        s.push_str("],\"underdetermined\":");
        match &self.underdetermined {
            Some(atom) => json_str(atom, &mut s),
            None => s.push_str("null"),
        }
        s.push_str(",\"unsat_core\":[");
        for (i, it) in self.unsat_core.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&it.origin, &mut s);
            s.push_str(",\"label\":");
            json_str(&it.label, &mut s);
            s.push('}');
        }
        s.push_str("],\"retract\":[");
        for (i, it) in self.retract.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&it.origin, &mut s);
            s.push_str(",\"label\":");
            json_str(&it.label, &mut s);
            s.push('}');
        }
        s.push_str("],\"hints\":[");
        for (i, h) in self.hints.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"a\":");
            json_str(&h.a, &mut s);
            s.push_str(",\"b\":");
            json_str(&h.b, &mut s);
            s.push_str(",\"reason\":");
            json_str(h.reason, &mut s);
            s.push('}');
        }
        s.push_str("],\"orphans\":[");
        for (i, o) in self.orphans.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&o.origin, &mut s);
            s.push_str(",\"atom\":");
            json_str(&o.atom, &mut s);
            let _ = write!(s, ",\"value\":{}", matches!(o.value, Value::True));
            s.push('}');
        }
        s.push_str("],\"unused_imports\":[");
        for (i, u) in self.unused_imports.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"file\":");
            json_str(&u.file, &mut s);
            s.push_str(",\"domain\":");
            json_str(&u.domain, &mut s);
            s.push_str(",\"alias\":");
            match &u.alias {
                Some(a) => json_str(a, &mut s),
                None => s.push_str("null"),
            }
            let _ = write!(s, ",\"line\":{}", u.line);
            s.push('}');
        }
        s.push_str("],\"placeholders\":[");
        for (i, p) in self.placeholders.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"key\":");
            json_str(&p.key, &mut s);
            let status = match p.status {
                PlaceholderStatus::Supplied => "supplied",
                PlaceholderStatus::DefaultUsed => "default",
                PlaceholderStatus::Unset => "unset",
            };
            s.push_str(",\"status\":");
            json_str(status, &mut s);
            match p.value {
                Some(v) => {
                    let _ = write!(s, ",\"value\":{v}");
                }
                None => s.push_str(",\"value\":null"),
            }
            s.push_str(",\"origin\":");
            match &p.origin {
                Some(o) => json_str(o, &mut s),
                None => s.push_str("null"),
            }
            s.push('}');
        }
        s.push_str("]}");
        s
    }
}

/// Push one derivation-trace step as a JSON object.
pub(crate) fn json_trace_step(step: &TraceStep, out: &mut String) {
    use core::fmt::Write as _;
    out.push_str("{\"atom\":");
    json_str(&step.atom, out);
    let _ = write!(out, ",\"value\":{}", matches!(step.value, Value::True));
    match &step.reason {
        TraceReason::Asserted(o) => {
            out.push_str(",\"how\":\"asserted\",");
            json_origin_fields(o, out);
            out.push_str(",\"from\":[]");
        }
        TraceReason::Derived { origin, from } => {
            out.push_str(",\"how\":\"derived\",");
            json_origin_fields(origin, out);
            out.push_str(",\"from\":");
            json_array(from, out);
        }
    }
    out.push('}');
}

pub(crate) fn status_name(s: Status) -> &'static str {
    match s {
        Status::Consistent => "CONSISTENT",
        Status::Underdetermined => "UNDERDETERMINED",
        Status::Warning => "WARNING",
        Status::Conflict => "CONFLICT",
    }
}

/// Push a JSON-escaped string literal (including the surrounding quotes).
pub(crate) fn json_str(value: &str, out: &mut String) {
    use core::fmt::Write as _;
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Push a JSON array of escaped strings.
pub(crate) fn json_array(items: &[String], out: &mut String) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_str(item, out);
    }
    out.push(']');
}

/// `"premise":..,"kind":..,"source":..,"line":..` (no braces).
pub(crate) fn json_origin_fields(o: &Origin, out: &mut String) {
    use core::fmt::Write as _;
    out.push_str("\"premise\":");
    match &o.premise {
        Some(name) => json_str(name, out),
        None => out.push_str("null"),
    }
    out.push_str(",\"kind\":");
    json_str(o.kind, out);
    out.push_str(",\"source\":");
    json_str(&o.source, out);
    let _ = write!(out, ",\"line\":{}", o.line);
}

/// Open an object `{` and write the origin fields.
pub(crate) fn json_origin(o: &Origin, out: &mut String) {
    out.push('{');
    json_origin_fields(o, out);
}
