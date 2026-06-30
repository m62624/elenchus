<!-- pi-code-planner:contracts:start -->
## Planner Contracts

### Purpose
Documents the elenchus-mcp crate — Model Context Protocol server exposing elenchus reasoning engine to AI agents.

### Parent
- `(root)`

### Child Index
- (none)

### Stable Contracts
- Transport: stdio, newline-delimited JSON-RPC 2.0 (one message per line), hand-rolled with serde_json (no MCP SDK).
- Server speaks: initialize, tools/list, tools/call, ping methods.
- Three tools exposed: elenchus_check (runs .vrf program), elenchus_version (reports engine version), elenchus_about (model-facing skill pointer).
- Layout: rpc.rs (JSON-RPC envelope + stdio loop), tools.rs (tool definitions + execution), messages.rs (model-facing strings).
- Thin wrapper — no engine logic reimplemented; delegates to elenchus-solver.

### Read First
- crates/elenchus-mcp/src/main.rs
- crates/elenchus-mcp/src/rpc.rs
- crates/elenchus-mcp/src/tools.rs
- crates/elenchus-mcp/src/messages.rs

### Do Not Touch Unless
- Tool definitions in tools.rs — AI agents depend on exact tool names and schemas
- Message strings in messages.rs — model-facing output must not change without version review
- JSON-RPC envelope in rpc.rs — protocol compliance

### Domain Details
- Module structure: main.rs (entry point, calls rpc::run()), rpc.rs (JSON-RPC handler + stdio loop), tools.rs (tool definitions + execution), messages.rs (model-facing strings)
- Public API: none (binary crate), but the MCP protocol surface is the contract: three tools with fixed names
- No feature flags — always requires std (it's a server binary)
- The MCP server is consumer-facing. Its behavior must be stable across releases because AI agents depend on exact tool names and message formats.
<!-- pi-code-planner:contracts:end -->
