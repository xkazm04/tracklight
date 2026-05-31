//! Scores: insert and list LLM-as-judge results.

use rusqlite::{params, Connection, Row};

use lighttrack_core::Score;

use super::util::{fmt_ts, parse_ts};
use crate::Result;

const COLS: &str = "id, project_id, event_id, rubric, value, max, pass, reasoning, \
    scored_by, cost_usd, created_at";

pub(super) fn insert(conn: &Connection, s: &Score) -> Result<()> {
    conn.execute(
        "INSERT INTO scores \
         (id, project_id, event_id, rubric, value, max, pass, reasoning, scored_by, cost_usd, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            s.id,
            s.project_id,
            s.event_id,
            s.rubric,
            s.value,
            s.max,
            s.pass.map(|b| b as i64),
            s.reasoning,
            s.scored_by,
            s.cost_usd,
            fmt_ts(s.created_at),
        ],
    )?;
    Ok(())
}

pub(super) fn list(conn: &Connection, project: Option<&str>, limit: usize) -> Result<Vec<Score>> {
    let raws: Vec<ScoreRaw> = if let Some(p) = project {
        let sql = format!(
            "SELECT {COLS} FROM scores WHERE project_id = ?1 ORDER BY created_at DESC LIMIT ?2"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![p, limit as i64], map_raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    } else {
        let sql = format!("SELECT {COLS} FROM scores ORDER BY created_at DESC LIMIT ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params![limit as i64], map_raw)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    raws.into_iter().map(from_raw).collect()
}

type ScoreRaw = (
    String,
    String,
    Option<String>,
    String,
    f64,
    f64,
    Option<i64>,
    Option<String>,
    String,
    Option<f64>,
    String,
);

fn map_raw(row: &Row) -> rusqlite::Result<ScoreRaw> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn from_raw(r: ScoreRaw) -> Result<Score> {
    Ok(Score {
        id: r.0,
        project_id: r.1,
        event_id: r.2,
        rubric: r.3,
        value: r.4,
        max: r.5,
        pass: r.6.map(|v| v != 0),
        reasoning: r.7,
        scored_by: r.8,
        cost_usd: r.9,
        created_at: parse_ts(&r.10)?,
    })
}
