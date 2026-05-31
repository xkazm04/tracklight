//! `lt-mcp` — a Model Context Protocol server exposing LightTrack's data to Claude Code / agents.
//!
//! Transport: MCP stdio = newline-delimited JSON-RPC 2.0 on stdin/stdout. Every byte on stdout is
//! protocol; all diagnostics go to stderr. The server is a thin HTTP client of the LightTrack API
//! (so it works locally today and against Cloud Run later), configured via env:
//!   LIGHTTRACK_URL (default http://127.0.0.1:8787), LIGHTTRACK_KEY (admin or project key).
//!
//! Tools (read-only): list_projects, get_cost_summary, query_events, get_limit_status, list_scores.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

struct Config {
    base: String,
    key: Option<String>,
    http: reqwest::blocking::Client,
}

fn main() {
    let cfg = Config {
        base: std::env::var("LIGHTTRACK_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into()),
        key: std::env::var("LIGHTTRACK_KEY").ok().filter(|s| !s.is_empty()),
        http: reqwest::blocking::Client::new(),
    };
    eprintln!("lt-mcp v{} started (base={})", env!("CARGO_PKG_VERSION"), cfg.base);

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("stdin read error: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("ignoring non-JSON line: {e}");
                continue;
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        eprintln!("-> {method}");

        match method {
            "initialize" => send_result(&mut out, id, initialize_result(&params)),
            "tools/list" => send_result(&mut out, id, tools_list()),
            "tools/call" => send_result(&mut out, id, handle_tool_call(&cfg, &params)),
            "ping" => send_result(&mut out, id, json!({})),
            // Capability probes we don't implement — answer with empties to avoid client noise.
            "resources/list" => send_result(&mut out, id, json!({ "resources": [] })),
            "resources/templates/list" => {
                send_result(&mut out, id, json!({ "resourceTemplates": [] }))
            }
            "prompts/list" => send_result(&mut out, id, json!({ "prompts": [] })),
            // Notifications carry no id and need no response.
            _ if id.is_none() => {}
            other => send_error(&mut out, id, -32601, &format!("method not found: {other}")),
        }
    }
}

fn initialize_result(params: &Value) -> Value {
    // Echo the client's protocol version for maximum compatibility.
    let pv = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": pv,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "lighttrack-mcp", "version": env!("CARGO_PKG_VERSION") }
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "list_projects",
                "description": "List LightTrack projects (requires an admin key in enforced mode).",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "get_cost_summary",
                "description": "Cost/usage rollup grouped by project + provider + model. Optionally filter by project.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "project": { "type": "string", "description": "project id to filter by" } }
                }
            },
            {
                "name": "query_events",
                "description": "Recent LLM call events (newest first). Optionally filter by project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "limit": { "type": "integer", "description": "max events (default 20)" }
                    }
                }
            },
            {
                "name": "get_limit_status",
                "description": "Evaluate a project's limit rules now; returns per-rule status and an overall throttle flag.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "project": { "type": "string" } },
                    "required": ["project"]
                }
            },
            {
                "name": "list_scores",
                "description": "Recent LLM-as-judge scores (newest first). Optionally filter by project.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": { "type": "string" },
                        "limit": { "type": "integer", "description": "max scores (default 20)" }
                    }
                }
            }
        ]
    })
}

fn handle_tool_call(cfg: &Config, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let outcome: Result<Value, String> = match name {
        "list_projects" => api_get(cfg, "/v1/projects"),
        "get_cost_summary" => api_get(cfg, &with_optional_project("/v1/costs", &args)),
        "query_events" => api_get(cfg, &list_path("/v1/events", &args)),
        "list_scores" => api_get(cfg, &list_path("/v1/scores", &args)),
        "get_limit_status" => match args.get("project").and_then(Value::as_str) {
            Some(p) => api_get(cfg, &format!("/v1/limits/status?project={p}")),
            None => Err("missing required argument: project".into()),
        },
        other => Err(format!("unknown tool: {other}")),
    };

    match outcome {
        Ok(v) => tool_text(&serde_json::to_string_pretty(&v).unwrap_or_default(), false),
        Err(e) => tool_text(&format!("error: {e}"), true),
    }
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": is_error })
}

fn with_optional_project(base: &str, args: &Value) -> String {
    match args.get("project").and_then(Value::as_str) {
        Some(p) => format!("{base}?project={p}"),
        None => base.to_string(),
    }
}

fn list_path(base: &str, args: &Value) -> String {
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20);
    let mut p = format!("{base}?limit={limit}");
    if let Some(proj) = args.get("project").and_then(Value::as_str) {
        p.push_str(&format!("&project={proj}"));
    }
    p
}

fn api_get(cfg: &Config, path: &str) -> Result<Value, String> {
    let mut req = cfg.http.get(format!("{}{}", cfg.base, path));
    if let Some(k) = &cfg.key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {}: {text}", status.as_u16()));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn send_result(out: &mut impl Write, id: Option<Value>, result: Value) {
    send(out, json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result }));
}

fn send_error(out: &mut impl Write, id: Option<Value>, code: i64, message: &str) {
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
