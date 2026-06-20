# elenchus-mcp

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

A [Model Context Protocol](https://modelcontextprotocol.io) server that exposes
the [elenchus](https://github.com/m62624/elenchus) consistency checker to AI agents.

Transport: **stdio, newline-delimited JSON-RPC 2.0** (one message per line).
Hand-rolled with `serde_json` — no MCP SDK dependency.

## CLI or MCP?

Both give an LLM the same elenchus output. Pick based on your setup:

- **CLI (`elenchus-cli`)** — works wherever you can run shell commands. No MCP
  configuration needed. If your harness supports shell tools, **use the CLI** —
  it's simpler to set up and works in every environment (Claude Code, CI, terminal).
- **MCP (`elenchus-mcp`)** — useful when your harness natively supports MCP and
  doesn't expose a shell, or when you'd rather wire up a single MCP server instead
  of a shell tool.

The **skill** ([`skill/SKILL.md`](../../skill/SKILL.md)) is adapted for both — it
works identically whether the agent calls elenchus via CLI or via the MCP tool.

## Tool

`elenchus_check` — check a `.vrf` program for logical consistency.

| Argument | Type | |
|----------|------|--|
| `program` | string (required) | the `.vrf` program: `FACT`/`NOT`/`ASSUME`, `PREMISE`/`RULE`, `CHECK` |
| `format` | `"human"` \| `"json"` (optional) | output format, default `"json"` |

The result is one of **CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT**.
Treat anything other than CONSISTENT as *not done*: add the missing facts or
rethink the premises, then call again — iterate until CONSISTENT.

## Run

```console
$ elenchus-mcp        # speaks JSON-RPC on stdin/stdout
```

Example session (each line is one JSON-RPC message):

```jsonc
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
← {"id":1,"jsonrpc":"2.0","result":{"capabilities":{"tools":{}},"protocolVersion":"2024-11-05","serverInfo":{"name":"elenchus","version":"0.6.0"}}}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT x a\nNOT x a\nCHECK x\n"}}}
← {"id":2,"jsonrpc":"2.0","result":{"content":[{"text":"{\"status\":\"CONFLICT\", …}","type":"text"}],"isError":false}}
```

## License

MIT — see [LICENSE](LICENSE).
