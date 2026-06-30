//! The three-valued Kleene truth value used throughout the forward pass.
use elenchus_compiler::Value;

/// Three-valued truth (Kleene). UNKNOWN is a first-class value, not hidden FALSE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V3 {
    /// Known true.
    True,
    /// Known false.
    False,
    /// Not asserted and not derivable — the absence of information, not falsity.
    Unknown,
}

impl V3 {
    /// Kleene negation: TRUE↔FALSE, and UNKNOWN stays UNKNOWN.
    pub(crate) fn not(self) -> V3 {
        match self {
            V3::True => V3::False,
            V3::False => V3::True,
            V3::Unknown => V3::Unknown,
        }
    }
}

/// Convert a three-valued model entry to a confident [`Value`] (UNKNOWN → `None`).
pub(crate) fn v3_to_value(v: V3) -> Option<Value> {
    match v {
        V3::True => Some(Value::True),
        V3::False => Some(Value::False),
        V3::Unknown => None,
    }
}
