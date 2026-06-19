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
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT x a\nCHECK x\n","format":"json"}}}"#,
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
    let pretty = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT svc built\nPREMISE gate:\n    WHEN svc built\n    THEN svc ready\nFACT svc ready\nCHECK svc\n","format":"json"}}}"#;
    let flat = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT svc built\nPREMISE gate:\nWHEN svc built\nTHEN svc ready\nFACT svc ready\nCHECK svc\n","format":"json"}}}"#;
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
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT x a\nNOT x a\nCHECK x\n"}}}"#,
    ]);
    let text = resps[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("CONFLICT"), "got: {text}");
}

#[test]
fn parse_error_is_a_tool_error() {
    let resps = roundtrip(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"elenchus_check","arguments":{"program":"FACT lonely\n"}}}"#,
    ]);
    assert_eq!(resps[0]["result"]["isError"], true);
}

#[test]
fn unknown_method_yields_jsonrpc_error() {
    let resps = roundtrip(&[r#"{"jsonrpc":"2.0","id":7,"method":"does/not/exist"}"#]);
    assert_eq!(resps[0]["error"]["code"], -32601);
}
