//! `elenchus-mcp` — a Model Context Protocol server exposing the elenchus
//! reasoning engine to AI agents.
//!
//! Transport: stdio, newline-delimited JSON-RPC 2.0 (one message per line).
//! Hand-rolled with `serde_json` (no MCP SDK). It speaks just enough of the
//! protocol — `initialize`, `tools/list`, `tools/call`, `ping` — to expose a
//! single tool, `elenchus_check`, which runs a `.vrf` program through the engine.

use std::io::{self, BufRead, Write};

use elenchus_solver::verify_source;
use serde_json::{Value, json};

const PROTOCOL_VERSION: &str = "2024-11-05";

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue; // ignore non-JSON lines
        };
        if let Some(response) = handle(&req) {
            let _ = writeln!(out, "{response}");
            let _ = out.flush();
        }
    }
}

/// Dispatch a JSON-RPC request. Returns `None` for notifications (no `id`) and
/// for anything that should not produce a reply.
fn handle(req: &Value) -> Option<Value> {
    let method = req.get("method")?.as_str()?;
    let id = req.get("id").cloned(); // absent ⇒ notification ⇒ no reply

    match method {
        "initialize" => id.map(|id| {
            result(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "elenchus", "version": env!("CARGO_PKG_VERSION") }
                }),
            )
        }),
        "notifications/initialized" => None,
        "ping" => id.map(|id| result(id, json!({}))),
        "tools/list" => id.map(|id| result(id, json!({ "tools": [tool_def()] }))),
        "tools/call" => id.map(|id| tools_call(id, req.get("params"))),
        // Unknown method: error only for requests (notifications are ignored).
        _ => id.map(|id| error(id, -32601, "method not found")),
    }
}

fn result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool result carrying text. `is_error` follows the MCP convention: tool-level
/// failures are reported in the result (not as a JSON-RPC error) so the model can
/// read and react to them.
fn tool_result(id: Value, text: String, is_error: bool) -> Value {
    result(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }),
    )
}

fn tool_def() -> Value {
    json!({
        "name": "elenchus_check",
        "description": "Check an elenchus `.vrf` program (facts, axioms, rules, checks) for \
    logical consistency. Returns one of CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT with \
    details and an exit code. Treat WARNING, UNDERDETERMINED and CONFLICT as NOT done: add the \
    missing facts or rethink the axioms, then call again — iterate until the result is CONSISTENT.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "program": {
                    "type": "string",
                    "description": "The .vrf program text: FACT/NOT assertions, AXIOM/RULE first principles, and a CHECK."
                },
                "format": {
                    "type": "string",
                    "enum": ["human", "json"],
                    "description": "Output format. Default \"json\"."
                }
            },
            "required": ["program"]
        }
    })
}

fn tools_call(id: Value, params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return error(id, -32602, "missing params");
    };
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != "elenchus_check" {
        return tool_result(id, format!("unknown tool: {name}"), true);
    }
    let args = params.get("arguments");
    let Some(program) = args.and_then(|a| a.get("program")).and_then(Value::as_str) else {
        return tool_result(id, "missing required argument: program".into(), true);
    };
    let format = args
        .and_then(|a| a.get("format"))
        .and_then(Value::as_str)
        .unwrap_or("json");

    match verify_source("<mcp>", program) {
        Ok(report) => {
            let text = if format == "human" {
                format!("{report}")
            } else {
                report.to_json()
            };
            tool_result(id, text, false)
        }
        Err(e) => tool_result(id, e.to_string(), true),
    }
}
