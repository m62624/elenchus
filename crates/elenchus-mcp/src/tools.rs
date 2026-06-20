//! The three tools the server exposes — `elenchus_check`, `elenchus_version` and
//! `elenchus_about` — their schema definitions and the `tools/call` executor.
//! Descriptions come from [`crate::messages`]; envelopes from [`crate::rpc`].

use elenchus_solver::{CompileError, verify_source};
use serde_json::{Value, json};

use crate::{messages, rpc};

/// Every tool definition, in the order `tools/list` advertises them.
pub fn definitions() -> Vec<Value> {
    vec![check_def(), version_def(), about_def()]
}

/// `elenchus_check` — run a `.vrf` program through the engine.
fn check_def() -> Value {
    json!({
        "name": "elenchus_check",
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
                "max_errors": {
                    "type": "integer",
                    "minimum": 0,
                    "description": messages::CHECK_ARG_MAX_ERRORS
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
    json!({
        "name": "elenchus_version",
        "description": messages::VERSION_TOOL,
        "inputSchema": { "type": "object", "properties": {} }
    })
}

/// `elenchus_about` — a pointer to the companion skill for agents that reached
/// this server without it. No version here; that is `elenchus_version`.
fn about_def() -> Value {
    json!({
        "name": "elenchus_about",
        "description": messages::ABOUT_TOOL,
        "inputSchema": { "type": "object", "properties": {} }
    })
}

/// Execute a `tools/call`: route by tool name, then hand off. A missing `params`
/// is a JSON-RPC error; an unknown tool is a tool-level error (`isError`).
pub fn call(id: Value, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return rpc::error(id, -32602, "missing params");
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");

    match name {
        "elenchus_version" => {
            rpc::tool_result(id, format!("elenchus {}", env!("CARGO_PKG_VERSION")), false)
        }
        "elenchus_about" => rpc::tool_result(id, messages::ABOUT_TOOL.to_string(), false),
        "elenchus_check" => check(id, params.get("arguments")),
        other => rpc::tool_result(id, format!("unknown tool: {other}"), true),
    }
}

/// The `elenchus_check` body: pull `program` (required) and `format` (default
/// `"json"`), run the engine, and return the human or JSON report. A missing
/// `program` or a parse/compile error is a tool-level error (`isError`).
fn check(id: Value, args: Option<&Value>) -> Value {
    let Some(program) = args.and_then(|a| a.get("program")).and_then(Value::as_str) else {
        return rpc::tool_result(id, "missing required argument: program".into(), true);
    };
    let format = args
        .and_then(|a| a.get("format"))
        .and_then(Value::as_str)
        .unwrap_or("json");
    let max_errors = args
        .and_then(|a| a.get("max_errors"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;

    match verify_source("<mcp>", program) {
        Ok(report) => {
            let text = if format == "human" {
                format!("{report}")
            } else {
                report.to_json()
            };
            rpc::tool_result(id, text, false)
        }
        // Syntax errors get the full diagnostic blocks (capped by `max_errors`).
        // `rpc::tool_result` carries the whole multi-line block as one JSON
        // string, which serde_json escapes — the wire stays valid JSON.
        Err(CompileError::Parse(diag)) => {
            let limit = (max_errors > 0).then_some(max_errors);
            rpc::tool_result(id, diag.render(limit), true)
        }
        Err(other) => rpc::tool_result(id, other.to_string(), true),
    }
}
