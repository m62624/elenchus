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
    ]);

    assert_eq!(resps.len(), 3, "the notification must not get a reply");
    assert_eq!(resps[0]["result"]["serverInfo"]["name"], "elenchus");
    assert_eq!(resps[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(resps[1]["result"]["tools"][0]["name"], "elenchus_check");

    let text = resps[2]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("CONSISTENT"), "got: {text}");
    assert_eq!(resps[2]["result"]["isError"], false);
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
