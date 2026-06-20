//! `elenchus-mcp` — a Model Context Protocol server exposing the elenchus
//! reasoning engine to AI agents.
//!
//! Transport: stdio, newline-delimited JSON-RPC 2.0 (one message per line).
//! Hand-rolled with `serde_json` (no MCP SDK). It speaks just enough of the
//! protocol — `initialize`, `tools/list`, `tools/call`, `ping` — to expose three
//! tools: `elenchus_check`, which runs a `.vrf` program through the engine;
//! `elenchus_version`, which reports the engine version for skill-version checks;
//! and `elenchus_about`, a model-facing note pointing at the companion skill.
//!
//! Layout, so protocol, behavior and wording are each editable in isolation:
//! `rpc` owns the JSON-RPC envelope and the stdio loop, `tools` the three tool
//! definitions and their execution, and `messages` every model-facing string.

mod messages;
mod rpc;
mod tools;

fn main() {
    rpc::run();
}
