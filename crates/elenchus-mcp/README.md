# elenchus-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server that exposes
the [elenchus](https://github.com/m62624/elenchus) reasoning engine to AI agents.

Transport: **stdio, newline-delimited JSON-RPC 2.0** (one message per line).
Hand-rolled with `serde_json` — no MCP SDK dependency.

## Tool

`elenchus_check` — check a `.vrf` program for logical consistency.

| Argument | Type | |
|----------|------|--|
| `program` | string (required) | the `.vrf` program: `FACT`/`NOT`, `AXIOM`/`RULE`, `CHECK` |
| `format` | `"human"` \| `"json"` (optional) | output format, default `"json"` |

The result is one of **CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT**.
Treat anything other than CONSISTENT as *not done*: add the missing facts or
rethink the axioms, then call again — iterate until CONSISTENT.

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

MIT — see the [workspace LICENSE](../../LICENSE).
