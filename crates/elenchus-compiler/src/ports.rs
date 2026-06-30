//! Port references: the parsed shape an external value binds to, and its label.
use alloc::string::{String, ToString};

/// A declared `VAR` port, keyed in the compiler by `(domain, name)`: its optional
/// `DEFAULT` fallback and where it was declared (for provenance on the synthetic
/// fact and the placeholders report).
pub(crate) struct PortDecl {
    pub(crate) default: Option<bool>,
    pub(crate) source: String,
    pub(crate) line: u32,
}

/// A parsed reference an external value binds: an optional canonical `domain`, a
/// `subject`, and optional `predicate`/`object`. A lone subject (predicate `None`)
/// names a `VAR` port; a multi-word ref names an atom an external value asserts.
/// `domain == None` means "search every domain" (the historical bare-name match);
/// `Some(d)` pins it to that domain. Built from a `PROVIDE` atom (alias resolved to
/// a canonical domain) or parsed from a CLI/API key by [`parse_port_ref`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PortRef {
    pub(crate) domain: Option<String>,
    pub(crate) subject: String,
    pub(crate) predicate: Option<String>,
    pub(crate) object: Option<String>,
}

impl PortRef {
    /// The human label used in error messages — the key the way it was written
    /// (`engine has_fuel`, `self.has_vision`).
    pub(crate) fn label(&self) -> String {
        let mut s = String::new();
        if let Some(d) = &self.domain {
            s.push_str(d);
            s.push('.');
        }
        s.push_str(&self.subject);
        if let Some(p) = &self.predicate {
            s.push(' ');
            s.push_str(p);
        }
        if let Some(o) = &self.object {
            s.push(' ');
            s.push_str(o);
        }
        s
    }
}

/// Parse a CLI/API key (`db_ready`, `self.has_vision`, `engine has_fuel`) into a
/// [`PortRef`]. A leading `domain.` (a `.` inside the first whitespace token)
/// becomes the domain; the rest splits on whitespace into `subject [predicate
/// [object]]`. `.` is not an identifier character, so the split is unambiguous and
/// backward-compatible with the bare names used before qualification existed.
pub(crate) fn parse_port_ref(key: &str) -> PortRef {
    let mut words = key.split_whitespace();
    let first = words.next().unwrap_or("");
    let (domain, subject) = match first.split_once('.') {
        Some((d, s)) => (Some(d.to_string()), s.to_string()),
        None => (None, first.to_string()),
    };
    PortRef {
        domain,
        subject,
        predicate: words.next().map(str::to_string),
        object: words.next().map(str::to_string),
    }
}
