//! The three tools the server exposes — `elenchus_check`, `elenchus_version` and
//! `elenchus_about` — their schema definitions and the `tools/call` executor.
//! Descriptions come from [`crate::messages`]; envelopes from [`crate::rpc`].

use elenchus_solver::{
    CompileError, MemoryResolver, PortBinding, read_data_bindings, verify_source_with, verify_with,
};
use serde_json::{Value, json};

use crate::{messages, rpc};

/// The tool names, shared by each definition and the `tools/call` dispatcher so
/// the advertised name and the routed name can never drift apart.
const CHECK: &str = "elenchus_check";
const VERSION: &str = "elenchus_version";
const ABOUT: &str = "elenchus_about";

/// Every tool definition, in the order `tools/list` advertises them.
pub fn definitions() -> Vec<Value> {
    vec![check_def(), version_def(), about_def()]
}

/// A no-argument tool definition: a name, a description, and an empty input
/// schema. Shared by the tools that take no parameters.
fn simple_def(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": {} }
    })
}

/// `elenchus_check` — run a `.vrf` program through the engine.
fn check_def() -> Value {
    json!({
        "name": CHECK,
        "description": messages::CHECK_TOOL,
        "inputSchema": {
            "type": "object",
            "properties": {
                "program": { "type": "string", "description": messages::CHECK_ARG_PROGRAM },
                "format": {
                    "type": "string",
                    "enum": ["human", "json"],
                    "description": messages::CHECK_ARG_FORMAT
                },
                "max_classes": {
                    "type": "integer",
                    "minimum": 0,
                    "description": messages::CHECK_ARG_MAX_CLASSES
                },
                "max_per_class": {
                    "type": "integer",
                    "minimum": 0,
                    "description": messages::CHECK_ARG_MAX_PER_CLASS
                },
                "values": {
                    "type": "object",
                    "additionalProperties": { "type": "boolean" },
                    "description": messages::CHECK_ARG_VALUES
                },
                "files": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": messages::CHECK_ARG_FILES
                },
                "data": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": messages::CHECK_ARG_DATA
                }
            },
            "required": ["program"]
        }
    })
}

/// `elenchus_version` — the MCP analog of `elenchus --version`, so a model can
/// read the running engine version (it cannot see `initialize`'s
/// `serverInfo.version`) and compare it to the version its skill targets.
fn version_def() -> Value {
    simple_def(VERSION, messages::VERSION_TOOL)
}

/// `elenchus_about` — a pointer to the companion skill for agents that reached
/// this server without it. No version here; that is `elenchus_version`.
fn about_def() -> Value {
    simple_def(ABOUT, messages::ABOUT_TOOL)
}

/// Execute a `tools/call`: route by tool name, then hand off. A missing `params`
/// is a JSON-RPC error; an unknown tool is a tool-level error (`isError`).
pub fn call(id: Value, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return rpc::error(id, -32602, "missing params");
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");

    match name {
        VERSION => rpc::tool_result(id, format!("elenchus {}", env!("CARGO_PKG_VERSION")), false),
        ABOUT => rpc::tool_result(id, messages::ABOUT_TOOL.to_string(), false),
        CHECK => check(id, params.get("arguments")),
        other => rpc::tool_result(id, format!("unknown tool: {other}"), true),
    }
}

/// The `elenchus_check` body: pull `program` (required) and `format` (default
/// `"json"`), run the engine, and return the human or JSON report. A missing
/// `program` or a parse/compile error is a tool-level error (`isError`).
///
/// `program` is the entry source. Two optional surfaces mirror the CLI's reach so
/// the three transports behave identically: `files` (an in-memory `{ path: text }`
/// import set, the resolver-less server's stand-in for the filesystem, enabling
/// IMPORT / multi-domain) and `data` (`{ name: PROVIDE-only text }`, the analog of
/// `--data`). `values` + `data` both feed VAR ports.
fn check(id: Value, args: Option<&Value>) -> Value {
    let Some(program) = args.and_then(|a| a.get("program")).and_then(Value::as_str) else {
        return rpc::tool_result(id, "missing required argument: program".into(), true);
    };
    let format = args
        .and_then(|a| a.get("format"))
        .and_then(Value::as_str)
        .unwrap_or("json");
    let arg_limit = |name: &str| {
        let n = args
            .and_then(|a| a.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        (n > 0).then_some(n)
    };
    // Port values: inline `values` ({ port: bool }) plus `data` ({ name: PROVIDE
    // text }). A bad data file (a non-PROVIDE statement) is a tool error.
    let inputs = match collect_inputs(args) {
        Ok(inputs) => inputs,
        Err(e) => return rpc::tool_result(id, e.to_string(), true),
    };

    // IMPORT: when `files` is present, resolve the graph through an in-memory store
    // (program registered as the `<mcp>` root); otherwise it is a single source.
    let result = match args.and_then(|a| a.get("files")).and_then(Value::as_object) {
        Some(files) if !files.is_empty() => {
            let mut resolver = MemoryResolver::new();
            for (path, content) in files {
                if let Some(text) = content.as_str() {
                    resolver.add(path, text);
                }
            }
            // Add the root last so a stray `files["<mcp>"]` can never shadow it.
            resolver.add("<mcp>", program);
            verify_with("<mcp>", &resolver, &inputs)
        }
        _ => verify_source_with("<mcp>", program, &inputs),
    };

    match result {
        Ok(report) => {
            let text = if format == "human" {
                format!("{report}")
            } else {
                report.to_json()
            };
            rpc::tool_result(id, text, false)
        }
        // Syntax errors get the grouped diagnostic blocks (capped by the two
        // limits). `rpc::tool_result` carries the whole multi-line block as one
        // JSON string, which serde_json escapes — the wire stays valid JSON.
        Err(CompileError::Parse(diag)) => {
            let text = diag.render(arg_limit("max_classes"), arg_limit("max_per_class"));
            rpc::tool_result(id, text, true)
        }
        Err(other) => rpc::tool_result(id, other.to_string(), true),
    }
}

/// Merge `values` (inline `{ port: bool }`, origin `api`) and `data` (`{ name:
/// PROVIDE-only text }`, parsed like a `--data` file) into one input list. A
/// non-boolean `values` entry is skipped; a non-`PROVIDE` statement in a `data`
/// source is a compile error.
fn collect_inputs(args: Option<&Value>) -> Result<Vec<(String, PortBinding)>, CompileError> {
    let mut inputs: Vec<(String, PortBinding)> = Vec::new();
    if let Some(m) = args
        .and_then(|a| a.get("values"))
        .and_then(Value::as_object)
    {
        for (k, v) in m {
            if let Some(value) = v.as_bool() {
                inputs.push((
                    k.clone(),
                    PortBinding {
                        value,
                        origin: "api".to_string(),
                    },
                ));
            }
        }
    }
    if let Some(m) = args.and_then(|a| a.get("data")).and_then(Value::as_object) {
        for (name, content) in m {
            if let Some(src) = content.as_str() {
                inputs.extend(read_data_bindings(name, src)?);
            }
        }
    }
    Ok(inputs)
}
