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
    CompileError, PortBinding, Report, Resolver, SolveOptions, VerifyError, normalize_import_path,
    read_data_bindings, verify_opts, verify_source_opts,
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
/// diagnostic block (capped by the two limits); any other error — including a
/// `max_conflicts` budget abort, which explicitly carries **no verdict** —
/// becomes its message (wasm has no error channel besides the returned string).
fn render(
    result: Result<Report, VerifyError>,
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
        Err(VerifyError::Compile(CompileError::Parse(diag))) => {
            diag.render(limit(max_classes), limit(max_per_class))
        }
        Err(other) => other.to_string(),
    }
}

/// The wasm spelling of [`SolveOptions`]: absent = unlimited (the normal case).
/// See the CLI `--max-conflicts` help — a safety valve for pathological inputs,
/// not something to set on an everyday check.
fn solve_options(max_conflicts: Option<u32>) -> SolveOptions {
    SolveOptions {
        max_conflicts: max_conflicts.map(u64::from),
    }
}

/// Iterate a JS object's own enumerable entries as `(string key, raw value)`
/// pairs, skipping any entry whose key is not a string. The one spelling of the
/// `Object.entries` → `(key, value)` decode shared by every port-input path.
fn object_entries(obj: &js_sys::Object) -> Vec<(String, JsValue)> {
    js_sys::Object::entries(obj)
        .iter()
        .filter_map(|entry| {
            let pair = js_sys::Array::from(&entry);
            pair.get(0).as_string().map(|key| (key, pair.get(1)))
        })
        .collect()
}

/// Decode an optional JS `Record<string, boolean>` of port values into the
/// engine's `(name, binding)` inputs (origin `"api"`). Non-boolean entries are
/// skipped. Only ever called from wasm (the host test build passes `None`).
fn decode_values(values: Option<js_sys::Object>) -> Vec<(String, PortBinding)> {
    let Some(obj) = values else {
        return Vec::new();
    };
    object_entries(&obj)
        .into_iter()
        .filter_map(|(name, value)| {
            value.as_bool().map(|value| {
                (
                    name,
                    PortBinding {
                        value,
                        origin: "api".to_string(),
                    },
                )
            })
        })
        .collect()
}

/// Merge inline `values` and `data` sources into one input list — the wasm analog
/// of the CLI's `--set` + `--data`. `data` is a `Record<string, string>` of
/// filename → PROVIDE-only `.vrf` text, parsed exactly like a CLI `--data` file
/// (origin `data:<name>`); a non-`PROVIDE` statement in one is a compile error.
/// `values` come first, then `data`; a disagreement between them is a port conflict
/// the engine rejects.
fn collect_inputs(
    values: Option<js_sys::Object>,
    data: Option<js_sys::Object>,
) -> Result<Vec<(String, PortBinding)>, CompileError> {
    let mut inputs = decode_values(values);
    if let Some(obj) = data {
        for (name, value) in object_entries(&obj) {
            if let Some(content) = value.as_string() {
                inputs.extend(read_data_bindings(&name, &content)?);
            }
        }
    }
    Ok(inputs)
}

/// Check a single `.vrf` program (inline text; `IMPORT` is not resolved — use
/// [`check_with_resolver`] for multi-file programs). Mirrors `elenchus_check`.
/// `values` supplies `VAR` port values as a `Record<string, boolean>`.
/// `max_conflicts` is the safety valve (normally omit; see [`solve_options`]) —
/// on abort the returned string is the error message, not a verdict.
#[wasm_bindgen]
pub fn check(
    program: &str,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
    values: Option<js_sys::Object>,
    data: Option<js_sys::Object>,
    max_conflicts: Option<u32>,
) -> String {
    match collect_inputs(values, data) {
        Ok(inputs) => render(
            verify_source_opts("<wasm>", program, &inputs, &solve_options(max_conflicts)),
            format,
            max_classes,
            max_per_class,
        ),
        Err(e) => render(Err(e.into()), format, max_classes, max_per_class),
    }
}

/// Check a `.vrf` program that may `IMPORT` other sources, resolving every import
/// through the JavaScript `read` callback: `read(path: string) -> string`
/// (synchronous; throw to signal "not found"). `root` is the entry path, passed
/// to `read` first. This is how Node `fs` (or any virtual store) backs `IMPORT`
/// inside wasm, where there is no filesystem.
// Positional wasm-bindgen FFI mirroring `check` + `root`/`read`; an options
// object would break every existing JS caller, so the arity is deliberate.
#[allow(clippy::too_many_arguments)]
#[wasm_bindgen]
pub fn check_with_resolver(
    root: &str,
    read: &js_sys::Function,
    format: Option<String>,
    max_classes: Option<u32>,
    max_per_class: Option<u32>,
    values: Option<js_sys::Object>,
    data: Option<js_sys::Object>,
    max_conflicts: Option<u32>,
) -> String {
    let resolver = JsResolver { read: read.clone() };
    match collect_inputs(values, data) {
        Ok(inputs) => render(
            verify_opts(root, &resolver, &inputs, &solve_options(max_conflicts)),
            format,
            max_classes,
            max_per_class,
        ),
        Err(e) => render(Err(e.into()), format, max_classes, max_per_class),
    }
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
        // Share the engine's OS-independent normalizer rather than `std::path`,
        // whose separator rules follow the *compile target* (wasm = Unix-like) and
        // would mishandle a Windows-style `sub\a.vrf` import. This makes the JS
        // resolver match the CLI's `FileResolver` byte-for-byte.
        normalize_import_path(base, relative)
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

/// The companion skill text (`SKILL.md`), so a consumer can persist it next to
/// the engine (e.g. into an agent's skills directory) without a second fetch.
/// The CLI/MCP "Run it" appendix (transport selection + the mandatory version
/// check) is stripped: in a wasm host there is one transport — an imported
/// function — and `skill()` always ships from the same release as the engine, so
/// that ceremony can never apply. See [`strip_wasm_excluded`].
#[wasm_bindgen]
pub fn skill() -> String {
    strip_wasm_excluded(SKILL_MD)
}

/// Remove the block fenced by `<!-- wasm-strip:begin -->` / `<!-- wasm-strip:end -->`
/// in the canonical `SKILL.md` (the fence markers included). Returns the input
/// unchanged if either marker is absent or they are out of order, so a canonical
/// skill without the fence still round-trips. `skill_version()` reads the raw
/// `SKILL_MD` const, so the `<!-- skill-version -->` marker living inside the
/// fenced block is unaffected by this strip.
fn strip_wasm_excluded(skill: &str) -> String {
    const BEGIN: &str = "<!-- wasm-strip:begin -->";
    const END: &str = "<!-- wasm-strip:end -->";
    let (Some(start), Some(end)) = (skill.find(BEGIN), skill.find(END)) else {
        return skill.to_string();
    };
    if end < start {
        return skill.to_string();
    }
    let head = skill[..start].trim_end();
    let tail = skill[end + END.len()..].trim_start();
    if tail.is_empty() {
        format!("{head}\n")
    } else {
        format!("{head}\n\n{tail}")
    }
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
        let json = check(
            "DOMAIN d\nFACT x a\nCHECK x",
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let human = check(
            "DOMAIN d\nFACT x a\nCHECK x",
            Some("human".to_string()),
            None,
            None,
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
        let out = check(
            "this is not a valid program",
            None,
            None,
            None,
            None,
            None,
            None,
        );
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
    fn max_conflicts_abort_is_an_error_string_not_a_verdict() {
        // A small pigeonhole (3 pigeons, 2 holes) — UNSAT that needs real search,
        // so a zero budget aborts before any verdict exists.
        let php = "DOMAIN php\n\
            PREMISE pigeon0:\n    ATLEAST\n        p0 in h0\n        p0 in h1\n\
            PREMISE pigeon1:\n    ATLEAST\n        p1 in h0\n        p1 in h1\n\
            PREMISE pigeon2:\n    ATLEAST\n        p2 in h0\n        p2 in h1\n\
            PREMISE hole0:\n    EXCLUSIVE\n        p0 in h0\n        p1 in h0\n        p2 in h0\n\
            PREMISE hole1:\n    EXCLUSIVE\n        p0 in h1\n        p1 in h1\n        p2 in h1\n\
            CHECK p0 BIDIRECTIONAL\n";
        let aborted = check(php, None, None, None, None, None, Some(0));
        assert!(
            aborted.contains("conflict budget exceeded"),
            "expected the budget error message, got: {aborted}"
        );
        assert!(
            !aborted.contains("exit_code"),
            "an abort must not look like a JSON verdict: {aborted}"
        );
        // Omitted (or generous) budget: the normal verdict, byte for byte.
        let free = check(php, None, None, None, None, None, None);
        let generous = check(php, None, None, None, None, None, Some(1_000_000));
        assert_eq!(free, generous);
        assert!(free.contains("exit_code"));
    }

    #[test]
    fn wasm_skill_omits_transport_appendix_but_keeps_the_marker_api() {
        let text = skill();
        // The CLI/MCP "Run it" ceremony is gone from the wasm-shipped skill…
        assert!(
            !text.contains("## Run it"),
            "the Run-it appendix must be stripped"
        );
        assert!(
            !text.contains("Step 0c"),
            "the version-check step must be stripped"
        );
        assert!(
            !text.contains("pick your transport"),
            "the transport-selection step must be stripped"
        );
        // …including the fence markers themselves.
        assert!(!text.contains("wasm-strip:begin"));
        assert!(!text.contains("wasm-strip:end"));
        // But the body a wasm consumer actually needs is intact…
        assert!(text.contains("name: elenchus"), "frontmatter must survive");
        assert!(
            text.contains("Reading the report"),
            "the DSL how-to must survive"
        );
        // …and the version API still works: it reads the raw const, not skill(),
        // so the marker inside the stripped block is still parseable even though
        // the human-readable text no longer carries it.
        assert!(
            !text.contains("skill-version:"),
            "the marker line rides in the stripped block"
        );
        assert!(
            !skill_version().is_empty(),
            "skill_version() still resolves from the const"
        );
    }

    #[test]
    fn strip_wasm_excluded_cuts_the_fenced_block_and_tolerates_absence() {
        let stripped = strip_wasm_excluded(
            "head\n\n<!-- wasm-strip:begin -->\nMIDDLE\n<!-- wasm-strip:end -->\n",
        );
        assert_eq!(stripped, "head\n");
        assert!(!stripped.contains("MIDDLE"));
        // No fence -> returned verbatim.
        assert_eq!(strip_wasm_excluded("no fence here"), "no fence here");
        // Content after the end fence is preserved.
        assert_eq!(
            strip_wasm_excluded("a\n<!-- wasm-strip:begin -->\nx\n<!-- wasm-strip:end -->\nb\n"),
            "a\n\nb\n",
        );
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
