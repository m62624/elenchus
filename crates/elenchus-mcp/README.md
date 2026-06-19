# elenchus-mcp

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

A [Model Context Protocol](https://modelcontextprotocol.io) server that exposes
the [elenchus](https://github.com/m62624/elenchus) consistency checker to AI agents.

Transport: **stdio, newline-delimited JSON-RPC 2.0** (one message per line).
Hand-rolled with `serde_json` — no MCP SDK dependency.

## Tool

`elenchus_check` — check a `.vrf` program for logical consistency.

| Argument | Type | |
|----------|------|--|
| `program` | string (required) | the `.vrf` program: `FACT`/`NOT`, `PREMISE`/`RULE`, `CHECK` |
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
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"elenchus","version":"0.1.0"}}}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT x a\nNOT x a\nCHECK x\n"}}}
← {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"status\":\"CONFLICT\",...}"}],"isError":false}}
```

## License

MIT — see [LICENSE](LICENSE).
