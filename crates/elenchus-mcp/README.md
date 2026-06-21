# elenchus-mcp

> ⚠️ **Experimental.** elenchus is mostly an AI-built experiment — written with the
> help of a small local model (Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf) and various Claude
> models, in roughly equal measure. Expect non-professional design choices, rough
> edges, broken behavior, or mistakes. Use it at your own risk.

A [Model Context Protocol](https://modelcontextprotocol.io) server that exposes
the [elenchus](https://github.com/m62624/elenchus) consistency checker to AI agents.

Transport: **stdio, newline-delimited JSON-RPC 2.0** (one message per line).
Hand-rolled with `serde_json` — no MCP SDK dependency.

## CLI or MCP — which one?

Both let an LLM run elenchus; the output is the same either way. The difference
is setup cost:

- **CLI (`elenchus-cli`)** — `elenchus-cli <file>` or `elenchus-cli --text "…"` from
  the shell. Works in every harness that can run shell commands (Claude Code, any
  CI pipeline, terminal). **Recommended: it needs no extra configuration, so if your
  harness can run shell commands, use the CLI.**
- **MCP server (`elenchus-mcp`)** — speaks stdio JSON-RPC. Worth the extra setup only
  when your harness natively supports MCP and you'd rather not (or can't) run a
  shell. Same output, more to configure.

The **skill** ([`skill/SKILL.md`](../../skill/SKILL.md)) is adapted for both — it
works identically whether the agent calls `elenchus-cli` via the CLI or via the MCP tool.

## Tool

`elenchus_check` — check a `.vrf` program for logical consistency.

| Argument | Type | |
|----------|------|--|
| `program` | string (required) | the `.vrf` program: `FACT`/`NOT`/`ASSUME`, `PREMISE`/`RULE`, `CHECK` |
| `format` | `"human"` \| `"json"` (optional) | output format, default `"json"` |
| `max_classes` | integer (optional) | on a syntax error, show at most this many error classes — one per keyword (`0` or omitted = all) |
| `max_per_class` | integer (optional) | on a syntax error, show at most this many places within each class (`0` or omitted = all) |

The result is one of **CONSISTENT / WARNING / UNDERDETERMINED / CONFLICT**.
Treat anything other than CONSISTENT as *not done*: add the missing facts or
rethink the premises, then call again — iterate until CONSISTENT.

A **syntax error** comes back as a tool error (`isError: true`): the `text`
field carries the full diagnostic, **grouped by class** (one per keyword) — the
correct syntax and an example shown once per class, with every offending place
listed beneath. Every error is reported in one pass. By default you get all of
them; `max_classes` and `max_per_class` independently cap the two dimensions
(both default to all). The whole multi-line block is a single JSON string, so the
wire stays valid JSON.

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
