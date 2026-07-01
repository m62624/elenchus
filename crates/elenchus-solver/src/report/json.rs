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
        // A rough capacity estimate (fixed skeleton + ~64 bytes per report entry)
        // avoids repeated reallocation as the string grows; it is only a hint —
        // the exact byte count is unaffected either way.
        let entries = self.conflicts.len()
            + self.warnings.len()
            + self.derived.len()
            + self.defeated.len()
            + self.unsat_core.len()
            + self.retract.len()
            + self.hints.len()
            + self.orphans.len()
            + self.unused_imports.len()
            + self.placeholders.len();
        let mut s = String::with_capacity(256 + entries * 64);
        let _ = write!(s, "{{\"status\":");
        status_name(self.status).write_json(&mut s);
        let _ = write!(s, ",\"exit_code\":{}", self.exit_code());

        s.push_str(",\"conflicts\":[");
        for (i, c) in self.conflicts.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&c.origin, &mut s);
            s.push_str(",\"atoms\":");
            c.atoms.write_json(&mut s);
            s.push_str(",\"trace\":[");
            for (j, step) in c.trace.iter().enumerate() {
                if j > 0 {
                    s.push(',');
                }
                step.write_json(&mut s);
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
            w.blocked_by.write_json(&mut s);
            s.push_str(",\"hint\":");
            match &w.hint {
                Some(h) => h.write_json(&mut s),
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
            d.atom.write_json(&mut s);
            let _ = write!(s, ",\"value\":{}", matches!(d.value, Value::True));
            s.push('}');
        }
        s.push_str("],\"defeated\":[");
        for (i, d) in self.defeated.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push('{');
            json_origin_fields(&d.origin, &mut s);
            s.push_str(",\"consequent\":");
            d.consequent.write_json(&mut s);
            s.push_str(",\"blocked_by\":");
            d.blocked_by.write_json(&mut s);
            s.push('}');
        }
        s.push_str("],\"underdetermined\":");
        match &self.underdetermined {
            Some(atom) => atom.write_json(&mut s),
            None => s.push_str("null"),
        }
        s.push_str(",\"unsat_core\":[");
        for (i, it) in self.unsat_core.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&it.origin, &mut s);
            s.push_str(",\"label\":");
            it.label.write_json(&mut s);
            s.push('}');
        }
        s.push_str("],\"retract\":[");
        for (i, it) in self.retract.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&it.origin, &mut s);
            s.push_str(",\"label\":");
            it.label.write_json(&mut s);
            s.push('}');
        }
        s.push_str("],\"hints\":[");
        for (i, h) in self.hints.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"a\":");
            h.a.write_json(&mut s);
            s.push_str(",\"b\":");
            h.b.write_json(&mut s);
            s.push_str(",\"reason\":");
            h.reason.write_json(&mut s);
            s.push('}');
        }
        s.push_str("],\"orphans\":[");
        for (i, o) in self.orphans.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            json_origin(&o.origin, &mut s);
            s.push_str(",\"atom\":");
            o.atom.write_json(&mut s);
            let _ = write!(s, ",\"value\":{}", matches!(o.value, Value::True));
            s.push('}');
        }
        s.push_str("],\"unused_imports\":[");
        for (i, u) in self.unused_imports.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str("{\"file\":");
            u.file.write_json(&mut s);
            s.push_str(",\"domain\":");
            u.domain.write_json(&mut s);
            s.push_str(",\"alias\":");
            match &u.alias {
                Some(a) => a.write_json(&mut s),
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
            p.key.write_json(&mut s);
            let status = match p.status {
                PlaceholderStatus::Supplied => "supplied",
                PlaceholderStatus::DefaultUsed => "default",
                PlaceholderStatus::Unset => "unset",
            };
            s.push_str(",\"status\":");
            status.write_json(&mut s);
            match p.value {
                Some(v) => {
                    let _ = write!(s, ",\"value\":{v}");
                }
                None => s.push_str(",\"value\":null"),
            }
            s.push_str(",\"origin\":");
            match &p.origin {
                Some(o) => o.write_json(&mut s),
                None => s.push_str("null"),
            }
            s.push('}');
        }
        s.push_str("]}");
        s
    }
}

/// Append a value's JSON encoding to `out`. Hand-rolled (no serde) so the crate
/// stays dependency-free and `no_std` — one trait so every leaf (strings, string
/// arrays, trace steps) shares the same `.write_json(out)` spelling.
trait ToJson {
    fn write_json(&self, out: &mut String);
}

/// A JSON string literal, with the mandatory escapes (`\uXXXX` for controls).
impl ToJson for str {
    fn write_json(&self, out: &mut String) {
        use core::fmt::Write as _;
        out.push('"');
        for ch in self.chars() {
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
}

/// A JSON array of strings.
impl ToJson for [String] {
    fn write_json(&self, out: &mut String) {
        out.push('[');
        for (i, item) in self.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            item.write_json(out);
        }
        out.push(']');
    }
}

/// One derivation-trace step as a JSON object.
impl ToJson for TraceStep {
    fn write_json(&self, out: &mut String) {
        use core::fmt::Write as _;
        out.push_str("{\"atom\":");
        self.atom.write_json(out);
        let _ = write!(out, ",\"value\":{}", matches!(self.value, Value::True));
        match &self.reason {
            TraceReason::Asserted(o) => {
                out.push_str(",\"how\":\"asserted\",");
                json_origin_fields(o, out);
                out.push_str(",\"from\":[]");
            }
            TraceReason::Derived { origin, from } => {
                out.push_str(",\"how\":\"derived\",");
                json_origin_fields(origin, out);
                out.push_str(",\"from\":");
                from.write_json(out);
            }
        }
        out.push('}');
    }
}

pub(crate) fn status_name(s: Status) -> &'static str {
    match s {
        Status::Consistent => "CONSISTENT",
        Status::Underdetermined => "UNDERDETERMINED",
        Status::Warning => "WARNING",
        Status::Conflict => "CONFLICT",
    }
}

/// `"premise":..,"kind":..,"source":..,"line":..` (no braces).
pub(crate) fn json_origin_fields(o: &Origin, out: &mut String) {
    use core::fmt::Write as _;
    out.push_str("\"premise\":");
    match &o.premise {
        Some(name) => name.write_json(out),
        None => out.push_str("null"),
    }
    out.push_str(",\"kind\":");
    o.kind.write_json(out);
    out.push_str(",\"source\":");
    o.source.write_json(out);
    let _ = write!(out, ",\"line\":{}", o.line);
}

/// Open an object `{` and write the origin fields.
pub(crate) fn json_origin(o: &Origin, out: &mut String) {
    out.push('{');
    json_origin_fields(o, out);
}
