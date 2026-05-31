//! Events: ingest, list, single-event lookup, cost rollup, and rolling usage.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::Value;

use lighttrack_core::{LlmEvent, Operation, Provider, Status, TokenUsage};

use super::util::{fmt_ts, parse_enum, parse_ts};
use crate::{CostRow, Result, Usage};

const COLS: &str = "id, project_id, trace_id, span_id, parent_span_id, ts, provider, model, \
    operation, input_tokens, output_tokens, cached_input_tokens, reasoning_tokens, cost_usd, \
    latency_ms, status, error, input, output, tags, source, metadata";

pub(super) fn insert(conn: &Connection, ev: &LlmEvent) -> Result<()> {
    let tags = serde_json::to_string(&ev.tags)?;
    let metadata = if ev.metadata.is_null() {
        None
    } else {
        Some(serde_json::to_string(&ev.metadata)?)
    };
    let input = ev.input.as_ref().map(serde_json::to_string).transpose()?;
    let output = ev.output.as_ref().map(serde_json::to_string).transpose()?;
    conn.execute(
        "INSERT INTO events \
         (id, project_id, trace_id, span_id, parent_span_id, ts, provider, model, operation, \
          input_tokens, output_tokens, cached_input_tokens, reasoning_tokens, cost_usd, \
          latency_ms, status, error, input, output, tags, source, metadata) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22)",
        params![
            ev.id,
            ev.project_id,
            ev.trace_id,
            ev.span_id,
            ev.parent_span_id,
            fmt_ts(ev.ts),
            ev.provider.as_str(),
            ev.model,
            ev.operation.as_str(),
            ev.usage.input as i64,
            ev.usage.output as i64,
            ev.usage.cached_input.map(|v| v as i64),
            ev.usage.reasoning.map(|v| v as i64),
            ev.cost_usd,
            ev.latency_ms.map(|v| v as i64),
            ev.status.as_str(),
            ev.error,
            input,
            output,
            tags,
            ev.source,
            metadata,
        ],
    )?;
    Ok(())
}

pub(super) fn list(conn: &Connection, project: Option<&str>, limit: usize) -> Result<Vec<LlmEvent>> {
    let raws: Vec<RawEvent> = if let Some(p) = project {
        let sql = format!("SELECT {COLS} FROM events WHERE project_id = ?1 ORDER BY ts DESC LIMIT ?2");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![p, limit as i64], map_raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    } else {
        let sql = format!("SELECT {COLS} FROM events ORDER BY ts DESC LIMIT ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![limit as i64], map_raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    raws.into_iter().map(from_raw).collect()
}

pub(super) fn get(conn: &Connection, id: &str) -> Result<Option<LlmEvent>> {
    let sql = format!("SELECT {COLS} FROM events WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let raw = stmt.query_row(params![id], map_raw).optional()?;
    raw.map(from_raw).transpose()
}

pub(super) fn cost_summary(conn: &Connection, project: Option<&str>) -> Result<Vec<CostRow>> {
    let cols = "project_id, provider, model, COUNT(*) AS calls, \
        COALESCE(SUM(input_tokens),0) AS it, COALESCE(SUM(output_tokens),0) AS ot, \
        COALESCE(SUM(cost_usd),0.0) AS cost";
    let map = |row: &Row| -> rusqlite::Result<CostRow> {
        Ok(CostRow {
            project_id: row.get(0)?,
            provider: row.get(1)?,
            model: row.get(2)?,
            calls: row.get(3)?,
            input_tokens: row.get(4)?,
            output_tokens: row.get(5)?,
            cost_usd: row.get(6)?,
        })
    };
    let rows = if let Some(p) = project {
        let sql = format!(
            "SELECT {cols} FROM events WHERE project_id = ?1 \
             GROUP BY project_id, provider, model ORDER BY cost DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let v = stmt.query_map(params![p], map)?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    } else {
        let sql = format!(
            "SELECT {cols} FROM events GROUP BY project_id, provider, model ORDER BY cost DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let v = stmt.query_map([], map)?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    Ok(rows)
}

pub(super) fn usage_since(conn: &Connection, project: &str, since: DateTime<Utc>) -> Result<Usage> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(SUM(cost_usd),0.0), COUNT(*), \
         COALESCE(SUM(input_tokens + output_tokens),0) \
         FROM events WHERE project_id = ?1 AND ts >= ?2",
    )?;
    let usage = stmt.query_row(params![project, fmt_ts(since)], |row| {
        Ok(Usage {
            cost_usd: row.get(0)?,
            calls: row.get(1)?,
            tokens: row.get(2)?,
        })
    })?;
    Ok(usage)
}

/// Raw column values as stored, before reconstructing an `LlmEvent`.
struct RawEvent {
    id: String,
    project_id: String,
    trace_id: Option<String>,
    span_id: Option<String>,
    parent_span_id: Option<String>,
    ts: String,
    provider: String,
    model: String,
    operation: String,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    cost_usd: Option<f64>,
    latency_ms: Option<i64>,
    status: String,
    error: Option<String>,
    input: Option<String>,
    output: Option<String>,
    tags: Option<String>,
    source: Option<String>,
    metadata: Option<String>,
}

fn map_raw(row: &Row) -> rusqlite::Result<RawEvent> {
    Ok(RawEvent {
        id: row.get(0)?,
        project_id: row.get(1)?,
        trace_id: row.get(2)?,
        span_id: row.get(3)?,
        parent_span_id: row.get(4)?,
        ts: row.get(5)?,
        provider: row.get(6)?,
        model: row.get(7)?,
        operation: row.get(8)?,
        input_tokens: row.get(9)?,
        output_tokens: row.get(10)?,
        cached_input_tokens: row.get(11)?,
        reasoning_tokens: row.get(12)?,
        cost_usd: row.get(13)?,
        latency_ms: row.get(14)?,
        status: row.get(15)?,
        error: row.get(16)?,
        input: row.get(17)?,
        output: row.get(18)?,
        tags: row.get(19)?,
        source: row.get(20)?,
        metadata: row.get(21)?,
    })
}

fn from_raw(r: RawEvent) -> Result<LlmEvent> {
    let ts = parse_ts(&r.ts)?;
    let input = match r.input {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    };
    let output = match r.output {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    };
    let tags: Vec<String> = match r.tags {
        Some(s) => serde_json::from_str(&s)?,
        None => Vec::new(),
    };
    let metadata: Value = match r.metadata {
        Some(s) => serde_json::from_str(&s)?,
        None => Value::Null,
    };
    Ok(LlmEvent {
        id: r.id,
        project_id: r.project_id,
        trace_id: r.trace_id,
        span_id: r.span_id,
        parent_span_id: r.parent_span_id,
        ts,
        provider: parse_enum::<Provider>(&r.provider),
        model: r.model,
        operation: parse_enum::<Operation>(&r.operation),
        usage: TokenUsage {
            input: r.input_tokens as u64,
            output: r.output_tokens as u64,
            cached_input: r.cached_input_tokens.map(|v| v as u64),
            reasoning: r.reasoning_tokens.map(|v| v as u64),
        },
        cost_usd: r.cost_usd,
        latency_ms: r.latency_ms.map(|v| v as u64),
        status: parse_enum::<Status>(&r.status),
        error: r.error,
        input,
        output,
        tags,
        source: r.source,
        metadata,
    })
}
