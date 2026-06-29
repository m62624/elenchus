//! `elenchus-wasm` — a thin wasm-bindgen wrapper exposing the elenchus engine
//! (parse → compile → solve) to JavaScript as "program text in, JSON verdict
//! out".
//!
//! No engine logic is reimplemented here: it reuses the core `elenchus-solver`
//! pipeline verbatim and mirrors the `elenchus-mcp` tool surface
//! (`check` / `version` / `about`), adding `skill` accessors (so a consumer can
//! persist the companion skill next to the engine) and an `IMPORT` resolver
//! bridged to a JavaScript `read(path) -> string` callback.

use elenchus_solver::{
    CompileError, PortBinding, Report, Resolver, verify_source_with, verify_with,
};
use wasm_bindgen::prelude::*;

/// The companion skill (the whole DSL how-to + the verdict loop), embedded so a
/// consumer can persist it next to the engine without a second download. Single
/// source of truth: the repo-root `skill/SKILL.md`.
const SKILL_MD: &str = include_str!("../../../skill/SKILL.md");

/// Mirror of `elenchus-mcp`'s `ABOUT_TOOL` — a short, version-free pointer to the
/// skill (the version lives in [`version`]).
const ABOUT: &str = "elenchus checks whether a set of facts and first principles is \
logically consistent (a small three-valued SAT checker). You'll get markedly better results \
with the matching `elenchus` skill loaded — it carries the verdict loop (iterate until \
CONSISTENT), the DSL, and worked examples. Call `version()` for the engine version and load \
the skill that matches it (see `skill()`): https://github.com/m62624/elenchus";

/// Turn an optional cap into the `Option<usize>` the diagnostics renderer takes:
/// `0` or absent means "no limit", matching `elenchus-mcp`.
fn limit(n: Option<u32>) -> Option<usize> {
    match n {
        Some(v) if v > 0 => Some(v as usize),
        _ => None,
    }
}

/// Render a result the way `elenchus-mcp` does: a `Report` becomes JSON (or the
/// human report when `format == "human"`); a parse error becomes the grouped
/// diagnostic block (capped by the two limits); any other compile error becomes
/// its message.
fn render(
    result: Result<Report, CompileError>,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
) -> String {
    match result {
        Ok(report) => {
            if format.as_deref() == Some("human") {
                format!("{report}")
            } else {
                report.to_json()
            }
        }
        Err(CompileError::Parse(diag)) => diag.render(limit(max_classes), limit(max_per_class)),
        Err(other) => other.to_string(),
    }
}

/// Decode an optional JS `Record<string, boolean>` of port values into the
/// engine's `(name, binding)` inputs (origin `"api"`). Non-boolean entries are
/// skipped. Only ever called from wasm (the host test build passes `None`).
fn decode_values(values: Option<js_sys::Object>) -> Vec<(String, PortBinding)> {
    let mut out = Vec::new();
    if let Some(obj) = values {
        for entry in js_sys::Object::entries(&obj).iter() {
            let pair = js_sys::Array::from(&entry);
            if let (Some(name), Some(value)) = (pair.get(0).as_string(), pair.get(1).as_bool()) {
                out.push((
                    name,
                    PortBinding {
                        value,
                        origin: "api".to_string(),
                    },
                ));
            }
        }
    }
    out
}

/// Check a single `.vrf` program (inline text; `IMPORT` is not resolved — use
/// [`check_with_resolver`] for multi-file programs). Mirrors `elenchus_check`.
/// `values` supplies `VAR` port values as a `Record<string, boolean>`.
#[wasm_bindgen]
pub fn check(
    program: &str,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
    values: Option<js_sys::Object>,
) -> String {
    let inputs = decode_values(values);
    render(
        verify_source_with("<wasm>", program, &inputs),
        format,
        max_classes,
        max_per_class,
    )
}

/// Check a `.vrf` program that may `IMPORT` other sources, resolving every import
/// through the JavaScript `read` callback: `read(path: string) -> string`
/// (synchronous; throw to signal "not found"). `root` is the entry path, passed
/// to `read` first. This is how Node `fs` (or any virtual store) backs `IMPORT`
/// inside wasm, where there is no filesystem.
#[wasm_bindgen]
pub fn check_with_resolver(
    root: &str,
    read: &js_sys::Function,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
    values: Option<js_sys::Object>,
) -> String {
    let resolver = JsResolver { read: read.clone() };
    let inputs = decode_values(values);
    render(
        verify_with(root, &resolver, &inputs),
        format,
        max_classes,
        max_per_class,
    )
}

/// Bridges the engine's [`Resolver`] to a JS `read(path) -> string` callback.
/// Path normalization mirrors the host `FileResolver` (relative imports resolve
/// against the importing file's directory, with manual `..` handling, forward
/// slashes) so resolved paths — and the provenance recorded in the IR — match
/// the CLI's.
struct JsResolver {
    read: js_sys::Function,
}

impl Resolver for JsResolver {
    fn load(&self, path: &str) -> Result<String, CompileError> {
        match self.read.call1(&JsValue::NULL, &JsValue::from_str(path)) {
            Ok(value) => value.as_string().ok_or_else(|| {
                CompileError::ImportNotFound(format!("{path}: read() did not return a string"))
            }),
            Err(err) => {
                let detail = err
                    .as_string()
                    .unwrap_or_else(|| "read() threw".to_string());
                Err(CompileError::ImportNotFound(format!("{path}: {detail}")))
            }
        }
    }

    fn resolve(&self, base: &str, relative: &str) -> String {
        use std::path::{Component, Path, PathBuf};
        let parent = Path::new(base).parent().unwrap_or_else(|| Path::new("."));
        let joined = parent.join(relative);
        let mut out = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                c => out.push(c),
            }
        }
        out.to_string_lossy().replace('\\', "/")
    }
}

/// The running engine version, e.g. `"elenchus 0.9.1"` — the *engine*, not this
/// npm package's version. Mirrors `elenchus_version`; compare it to the skill's
/// `<!-- skill-version -->` marker (see [`skill_version`]).
#[wasm_bindgen]
pub fn version() -> String {
    format!("elenchus {}", elenchus_solver::VERSION)
}

/// A short pointer to the companion skill. Mirrors `elenchus_about`.
#[wasm_bindgen]
pub fn about() -> String {
    ABOUT.to_string()
}

/// The full companion skill text (`SKILL.md`), so a consumer can persist it next
/// to the engine (e.g. into an agent's skills directory) without a second fetch.
#[wasm_bindgen]
pub fn skill() -> String {
    SKILL_MD.to_string()
}

/// The skill's `<!-- skill-version: X -->` marker (the engine version the skill
/// targets), parsed from the embedded skill. Empty if the marker is absent.
#[wasm_bindgen]
pub fn skill_version() -> String {
    skill_version_of(SKILL_MD).unwrap_or_default()
}

/// Extract the `<!-- skill-version: X -->` marker value from skill text.
fn skill_version_of(skill: &str) -> Option<String> {
    let marker = "<!-- skill-version:";
    let start = skill.find(marker)? + marker.len();
    let rest = &skill[start..];
    let end = rest.find("-->")?;
    Some(rest[..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_reports_conflict_as_json() {
        let out = check(
            "DOMAIN d\nFACT x a\nNOT x a\nCHECK x",
            None,
            None,
            None,
            None,
        );
        assert!(
            out.contains("CONFLICT"),
            "expected a CONFLICT verdict, got: {out}"
        );
        assert!(
            out.contains("exit_code"),
            "expected machine JSON, got: {out}"
        );
    }

    #[test]
    fn check_human_format_differs_from_json() {
        let json = check("DOMAIN d\nFACT x a\nCHECK x", None, None, None, None);
        let human = check(
            "DOMAIN d\nFACT x a\nCHECK x",
            Some("human".to_string()),
            None,
            None,
            None,
        );
        assert!(json.contains("status"));
        assert_ne!(json, human);
    }

    #[test]
    fn version_reports_engine_not_package() {
        assert_eq!(version(), format!("elenchus {}", elenchus_solver::VERSION));
    }

    #[test]
    fn skill_text_and_marker_are_present() {
        assert!(skill().contains("name: elenchus"));
        // The marker is present and x.y.z-shaped. It is deliberately NOT asserted
        // equal to the engine version here: a release bumps the SKILL.md marker
        // and the crate version at different moments, so between releases they are
        // legitimately out of sync. The release-only CI job `skill-check` owns the
        // "marker == release version" check.
        let marker = skill_version();
        let core = marker.split('-').next().unwrap_or("");
        let parts: Vec<&str> = core.split('.').collect();
        assert!(
            parts.len() == 3
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
            "skill-version marker should be x.y.z, got: {marker:?}"
        );
    }

    #[test]
    fn check_syntax_error_is_not_a_json_verdict() {
        // A malformed program goes through the diagnostics renderer / error
        // message path, never the JSON report path.
        let out = check("this is not a valid program", None, None, None, None);
        assert!(
            !out.contains("exit_code"),
            "a syntax/compile error must not look like a JSON verdict: {out}"
        );
        assert!(!out.trim().is_empty());
    }

    #[test]
    fn limit_maps_zero_and_absent_to_none() {
        assert_eq!(limit(None), None);
        assert_eq!(limit(Some(0)), None);
        assert_eq!(limit(Some(3)), Some(3));
    }

    #[test]
    fn skill_version_of_parses_marker_and_tolerates_absence() {
        assert_eq!(
            skill_version_of("intro\n<!-- skill-version: 1.2.3 -->\nrest").as_deref(),
            Some("1.2.3"),
        );
        assert_eq!(skill_version_of("no marker here"), None);
    }
}
