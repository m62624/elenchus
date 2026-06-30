<!-- pi-code-planner:contracts:start -->
## Planner Contracts

### Purpose
Documents the elenchus-mcp crate — a Model Context Protocol server exposing the elenchus reasoning engine to AI agents. Covers JSON-RPC 2.0 over stdio, the three tools (elenchus_check, elenchus_version, elenchus_about), and message formatting.

### Parent
- `(root)`

### Child Index
- (none)

### Stable Contracts
- Transport: stdio, newline-delimited JSON-RPC 2.0 (one message per line), hand-rolled with serde_json (no MCP SDK).
- The server speaks initialize, tools/list, tools/call, and ping methods.
- Three tools exposed: elenchus_check (runs a .vrf program through the engine), elenchus_version (reports engine version for skill-version checks), elenchus_about (model-facing pointer to companion skill).
- Layout: rpc.rs owns the JSON-RPC envelope and stdio loop, tools.rs owns the three tool definitions and execution, messages.rs owns every model-facing string.
- The server is a thin wrapper — no engine logic is reimplemented; it delegates to elenchus-solver.

### Read First
- crates/elenchus-mcp/src/main.rs
- crates/elenchus-mcp/src/rpc.rs
- crates/elenchus-mcp/src/tools.rs
- crates/elenchus-mcp/src/messages.rs

### Do Not Touch Unless
- Tool definitions in tools.rs — consumers (AI agents) depend on exact tool names and schemas
- Message strings in messages.rs — model-facing output must not change without version review
- JSON-RPC envelope in rpc.rs — protocol compliance

### Domain Details
- Module structure: main.rs (entry point, calls rpc::run()), rpc.rs (JSON-RPC handler + stdio loop), tools.rs (tool definitions + execution), messages.rs (model-facing strings)
- Public API: none (binary crate), but the MCP protocol surface is the contract: three tools with fixed names
- No feature flags — always requires std (it's a server binary)
- The MCP server is a consumer-facing binary. Its behavior must be stable across releases because AI agents depend on exact tool names and message formats.
<!-- pi-code-planner:contracts:end -->
