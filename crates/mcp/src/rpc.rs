//! JSON-RPC 2.0 framing over stdio. stdout carries protocol bytes only; diagnostics go to stderr.

use std::io::Write;

use serde_json::{json, Value};

pub(crate) fn initialize_result(params: &Value) -> Value {
    // Echo the client's protocol version for maximum compatibility.
    let pv = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": pv,
        "capabilities": { "tools": {}, "prompts": {} },
        "serverInfo": { "name": "lighttrack-mcp", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// Wrap text as an MCP tool-call result (`content` + `isError`).
pub(crate) fn tool_text(text: &str, is_error: bool) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": is_error })
}

/// A rendered tool result: human-facing Markdown in `content` (what Claude Code shows + relays),
/// plus the exact raw object in `structuredContent` (arrays wrapped under `items`) for clients/agents
/// that consume structure. The Markdown carries full ids so follow-up calls work even where a client
/// drops `structuredContent`.
pub(crate) fn tool_rendered(markdown: &str, raw: &Value) -> Value {
    let structured = if raw.is_array() {
        json!({ "items": raw })
    } else {
        raw.clone()
    };
    json!({
        "content": [ { "type": "text", "text": markdown } ],
        "structuredContent": structured,
        "isError": false
    })
}

pub(crate) fn send_result(out: &mut impl Write, id: Option<Value>, result: Value) {
    send(out, json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result }));
}

pub(crate) fn send_error(out: &mut impl Write, id: Option<Value>, code: i64, message: &str) {
    send(
        out,
        json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "error": { "code": code, "message": message } }),
    );
}

fn send(out: &mut impl Write, msg: Value) {
    if writeln!(out, "{msg}").and_then(|_| out.flush()).is_err() {
        eprintln!("failed to write response");
    }
}
