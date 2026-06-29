//! Black-box tests of the MCP server: drive it over stdio JSON-RPC and check replies.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

/// Feed each request line to the server, return the parsed JSON-RPC replies.
fn roundtrip(requests: &[&str]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_elenchus-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn elenchus-mcp");
    {
        let stdin = child.stdin.as_mut().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
    } // drop stdin → EOF → server exits
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn initialize_list_and_call() {
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, // notification → no reply
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nFACT x a\nCHECK x\n","format":"json"}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"elenchus_version","arguments":{}}}"#,
    ]);

    assert_eq!(resps.len(), 4, "the notification must not get a reply");
    assert_eq!(resps[0]["result"]["serverInfo"]["name"], "elenchus");
    assert_eq!(resps[0]["result"]["protocolVersion"], "2024-11-05");
    // Both tools are advertised.
    assert_eq!(resps[1]["result"]["tools"][0]["name"], "elenchus_check");
    assert_eq!(resps[1]["result"]["tools"][1]["name"], "elenchus_version");

    let text = resps[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("CONSISTENT"), "got: {text}");
    assert_eq!(resps[2]["result"]["isError"], false);

    // elenchus_version returns the engine version, matching serverInfo.
    let version = resps[0]["result"]["serverInfo"]["version"]
        .as_str()
        .unwrap();
    let vtext = resps[3]["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(resps[3]["result"]["isError"], false);
    assert!(
        vtext.contains(version),
        "version tool `{vtext}` should contain {version}"
    );
}

#[test]
fn about_tool_is_listed_and_points_at_the_skill() {
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"elenchus_about","arguments":{}}}"#,
    ]);
    // The third advertised tool is the about/skill pointer.
    assert_eq!(resps[0]["result"]["tools"][2]["name"], "elenchus_about");
    let text = resps[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(resps[1]["result"]["isError"], false);
    assert!(
        text.contains("skill"),
        "about should mention the skill: {text}"
    );
    assert!(
        text.contains("github.com/m62624/elenchus"),
        "about should link the project: {text}"
    );
}

#[test]
fn program_whitespace_is_cosmetic() {
    // Indented (readable) vs flat (no-indent) — identical to the engine.
    let pretty = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nFACT svc built\nPREMISE gate:\n    WHEN svc built\n    THEN svc ready\nFACT svc ready\nCHECK svc\n","format":"json"}}}"#;
    let flat = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nFACT svc built\nPREMISE gate:\nWHEN svc built\nTHEN svc ready\nFACT svc ready\nCHECK svc\n","format":"json"}}}"#;
    let a = roundtrip(&[pretty]);
    let b = roundtrip(&[flat]);
    let ta = a[0]["result"]["content"][0]["text"].as_str().unwrap();
    let tb = b[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(ta.contains("CONSISTENT"), "got: {ta}");
    assert_eq!(ta, tb, "indentation must not change the report");
}

#[test]
fn conflict_program_is_reported() {
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nFACT x a\nNOT x a\nCHECK x\n"}}}"#,
    ]);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("CONFLICT"), "got: {text}");
}

#[test]
fn orphan_fact_rides_through_json_over_mcp() {
    // `lonely sits idle` is referenced by no premise/rule → an advisory orphan.
    // The JSON report must carry it in the `orphans` array (parsed back through
    // serde_json by `roundtrip`, so the wire stays valid), while the verdict
    // stays CONSISTENT and the call is not an error.
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nFACT lonely sits idle\nCHECK\n","format":"json"}}}"#,
    ]);
    assert_eq!(resps[0]["result"]["isError"], false);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    // The text field holds a JSON document — re-parse it and inspect `orphans`.
    let report: Value = serde_json::from_str(text).expect("report text must be valid JSON");
    assert_eq!(report["status"], "CONSISTENT");
    assert_eq!(report["exit_code"], 0);
    let orphans = report["orphans"].as_array().expect("orphans array");
    assert_eq!(orphans.len(), 1, "got: {text}");
    assert_eq!(orphans[0]["atom"], "d.lonely sits idle");
    assert_eq!(orphans[0]["kind"], "FACT");
    assert_eq!(orphans[0]["value"], true);
}

#[test]
fn values_supply_a_port_and_flip_the_verdict() {
    // The VAR port `k` drives the premise `WHEN k THEN x a` against `NOT x a`:
    // `{"k":false}` satisfies it (CONSISTENT), `{"k":true}` violates it (CONFLICT).
    // A raw string so each `\n` is the two-char JSON escape, not a real newline
    // (a literal newline would break the one-line JSON-RPC request).
    const PROG: &str = r"DOMAIN d\nVAR k\nNOT x a\nPREMISE g:\n    WHEN k\n    THEN x a\nCHECK\n";
    let req = |id: u32, values: &str| {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"elenchus_check","arguments":{{"program":"{PROG}","format":"json","values":{values}}}}}}}"#
        )
    };
    let resps = roundtrip(&[&req(1, r#"{"k":false}"#), &req(2, r#"{"k":true}"#)]);

    assert_eq!(resps[0]["result"]["isError"], false);
    let consistent: Value =
        serde_json::from_str(resps[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(consistent["status"], "CONSISTENT");
    // The placeholders section round-trips with the supplied value and origin.
    assert_eq!(consistent["placeholders"][0]["key"], "k");
    assert_eq!(consistent["placeholders"][0]["status"], "supplied");
    assert_eq!(consistent["placeholders"][0]["value"], false);

    let conflict: Value =
        serde_json::from_str(resps[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(conflict["status"], "CONFLICT");
}

#[test]
fn values_conflicting_with_in_file_provide_is_a_tool_error() {
    // `PROVIDE k: true` in the program and `{"k":false}` in `values` disagree → a
    // hard error surfaced as a tool error.
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nVAR k\nPROVIDE k: true\nFACT x a\nCHECK\n","values":{"k":false}}}}"#,
    ]);
    assert_eq!(resps[0]["result"]["isError"], true);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("set to two different values"), "got: {text}");
}

#[test]
fn files_resolve_imports_across_domains() {
    // The root IMPORTs `a.vrf` (a second domain) and asserts a fact into it that
    // contradicts that file's `NOT x p` → CONFLICT. Without `files` the same
    // program can't resolve the import and errors, so this proves `files` is what
    // wires IMPORT over MCP (the resolver-less transport).
    let with_files = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN r\nIMPORT \"a.vrf\"\nFACT a.x p\nCHECK\n","files":{"a.vrf":"DOMAIN a\nNOT x p\n"}}}}"#;
    let without_files = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN r\nIMPORT \"a.vrf\"\nFACT a.x p\nCHECK\n"}}}"#;
    let resps = roundtrip(&[with_files, without_files]);

    assert_eq!(resps[0]["result"]["isError"], false);
    let report: Value =
        serde_json::from_str(resps[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["status"], "CONFLICT");

    // No `files` → the import can't be resolved → a tool error.
    assert_eq!(resps[1]["result"]["isError"], true);
}

#[test]
fn files_resolve_a_windows_style_import_path() {
    // The program IMPORTs `sub\a.vrf` (Windows separators) but the `files` key is
    // `sub/a.vrf`: the shared normalizer makes them the same path, so the import
    // resolves and the contradiction fires. Proves MCP's MemoryResolver normalizes
    // paths the same on every host.
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN r\nIMPORT \"sub\\a.vrf\"\nFACT a.x p\nCHECK\n","files":{"sub/a.vrf":"DOMAIN a\nNOT x p\n"}}}}"#;
    let resps = roundtrip(&[req]);
    assert_eq!(resps[0]["result"]["isError"], false, "got: {:?}", resps[0]);
    let report: Value =
        serde_json::from_str(resps[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["status"], "CONFLICT");
}

#[test]
fn data_files_supply_port_values_over_mcp() {
    // The `data` map carries a PROVIDE file, exactly like the CLI's `--data`: it
    // drives the premise to CONFLICT and is reported with a `data:` origin.
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nVAR k\nNOT x a\nPREMISE g:\n    WHEN k\n    THEN x a\nCHECK\n","data":{"vals.vrf":"PROVIDE k: true\n"}}}}"#;
    let resps = roundtrip(&[req]);

    assert_eq!(resps[0]["result"]["isError"], false);
    let report: Value =
        serde_json::from_str(resps[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["status"], "CONFLICT");
    assert_eq!(report["placeholders"][0]["status"], "supplied");
    assert_eq!(report["placeholders"][0]["origin"], "data:vals.vrf");
}

#[test]
fn data_file_carrying_logic_is_a_tool_error() {
    // A `data` source may hold only PROVIDE lines; a FACT in it is a hard error.
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nVAR k\nFACT x a\nCHECK\n","data":{"bad.vrf":"PROVIDE k: true\nFACT y b\n"}}}}"#;
    let resps = roundtrip(&[req]);
    assert_eq!(resps[0]["result"]["isError"], true);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("data file may only contain PROVIDE"),
        "got: {text}"
    );
}

#[test]
fn data_paths_read_provide_files_from_disk() {
    // `data_paths` mode: the server reads a PROVIDE-only file off disk (like CLI
    // `--data <file>`) and it drives the premise to CONFLICT, reported with a
    // `data:<path>` origin — the filesystem analog of the `data` map above.
    let dir = env!("CARGO_TARGET_TMPDIR");
    let data = format!("{dir}/mcp-data-paths.vrf");
    std::fs::write(&data, "PROVIDE k: true\n").unwrap();
    // Build the request with serde_json so a Windows path (backslashes) escapes
    // into valid JSON rather than emitting invalid `\U`-style sequences.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "elenchus_check",
            "arguments": {
                "program": "DOMAIN d\nVAR k\nNOT x a\nPREMISE g:\n    WHEN k\n    THEN x a\nCHECK\n",
                "data_paths": [data],
                "format": "json"
            }
        }
    })
    .to_string();
    let resps = roundtrip(&[&req]);
    assert_eq!(resps[0]["result"]["isError"], false, "got: {:?}", resps[0]);
    let report: Value =
        serde_json::from_str(resps[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["status"], "CONFLICT");
    assert_eq!(report["placeholders"][0]["status"], "supplied");
    assert_eq!(report["placeholders"][0]["origin"], format!("data:{data}"));
}

#[test]
fn data_paths_missing_file_is_a_tool_error() {
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nVAR k\nFACT x a\nCHECK\n","data_paths":["/no/such/data.vrf"]}}}"#;
    let resps = roundtrip(&[req]);
    assert_eq!(resps[0]["result"]["isError"], true);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("/no/such/data.vrf"), "got: {text}");
}

#[test]
fn path_reads_a_file_and_resolves_imports_from_disk() {
    // `path` mode: the server reads the entry `.vrf` from disk and resolves its
    // IMPORT from disk too (like `elenchus-cli <file>`). The imported `NOT x p`
    // contradicts the asserted `FACT a.x p` → CONFLICT, proving disk resolution.
    let dir = env!("CARGO_TARGET_TMPDIR");
    let root = format!("{dir}/mcp-fs-root.vrf");
    let dep = format!("{dir}/mcp-fs-dep.vrf");
    std::fs::write(&dep, "DOMAIN a\nNOT x p\n").unwrap();
    std::fs::write(
        &root,
        "DOMAIN r\nIMPORT \"mcp-fs-dep.vrf\"\nFACT a.x p\nCHECK\n",
    )
    .unwrap();
    // Build the request with serde_json so a Windows path (backslashes) is escaped
    // into valid JSON; hand-splicing it would emit invalid `\U`-style escapes.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "elenchus_check",
            "arguments": { "path": root, "format": "json" }
        }
    })
    .to_string();
    let resps = roundtrip(&[&req]);
    assert_eq!(resps[0]["result"]["isError"], false, "got: {:?}", resps[0]);
    let report: Value =
        serde_json::from_str(resps[0]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["status"], "CONFLICT");
}

#[test]
fn program_and_path_together_is_a_tool_error() {
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nCHECK\n","path":"/nope.vrf"}}}"#;
    let resps = roundtrip(&[req]);
    assert_eq!(resps[0]["result"]["isError"], true);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("not both"), "got: {text}");
}

#[test]
fn neither_program_nor_path_is_a_tool_error() {
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"format":"json"}}}"#;
    let resps = roundtrip(&[req]);
    assert_eq!(resps[0]["result"]["isError"], true);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("missing entry"), "got: {text}");
}

#[test]
fn orphan_fact_renders_in_the_human_report_over_mcp() {
    // The `human` format: the advisory ORPHAN line reaches the text field.
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"DOMAIN d\nFACT lonely sits idle\nCHECK\n","format":"human"}}}"#,
    ]);
    assert_eq!(resps[0]["result"]["isError"], false);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("ORPHAN    FACT d.lonely sits idle"),
        "got: {text}"
    );
}

#[test]
fn parse_error_is_a_tool_error() {
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT a b c d\n"}}}"#,
    ]);
    assert_eq!(resps[0]["result"]["isError"], true);
    // The full diagnostic block (not a one-liner) arrives in the text field.
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("unexpected text after the FACT atom"),
        "got: {text}"
    );
}

#[test]
fn grouped_block_stays_valid_json_and_respects_max_per_class() {
    // Four syntax errors (three FACT, one NOT). The whole multi-line block —
    // newlines, quotes, `|`, `^` carets — must arrive as ONE JSON string.
    // `roundtrip` parses each reply with serde_json, so if the wire were broken
    // it would already have panicked; reaching the asserts proves valid JSON.
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT a b c d\nFACT a b c e\nFACT a b c f\nNOT a b c d\n","max_per_class":1}}}"#,
    ]);
    assert_eq!(resps[0]["result"]["isError"], true);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("RESULT: 4 syntax errors"), "got: {text}");
    assert!(text.contains('^'), "block should carry a caret: {text}");
    assert!(
        text.contains("... and 2 more FACT problems"),
        "max_per_class should cap: {text}"
    );
}

#[test]
fn max_classes_caps_classes_over_mcp() {
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT a b c d\nNOT a b c d\n","max_classes":1}}}"#,
    ]);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("... and 1 more class"), "got: {text}");
}

#[test]
fn all_syntax_errors_grouped_when_no_caps() {
    // Without caps every class and place comes back, no "more" footers.
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT a b c d\nFACT a b c e\nNOT a b c d\n"}}}"#,
    ]);
    assert_eq!(resps[0]["result"]["isError"], true);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("RESULT: 3 syntax errors"), "got: {text}");
    assert!(text.contains("FACT  (2 problems)"), "got: {text}");
    assert!(text.contains("NOT  (1 problem)"), "got: {text}");
    assert!(!text.contains("more"), "no caps → no footers: {text}");
}

#[test]
fn unknown_method_yields_jsonrpc_error() {
    let resps = roundtrip(&[r#"{"jsonrpc":"2.0","id":7,"method":"does/not/exist"}"#]);
    assert_eq!(resps[0]["error"]["code"], -32601);
}
