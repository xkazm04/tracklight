//! Postgres backend for LightTrack — implements the [`lighttrack_store::Store`] trait via `sqlx`,
//! so the same app runs on any managed Postgres (RDS / Cloud SQL / Azure DB / Neon / Supabase).
//!
//! The `Store` trait is synchronous (the SQLite backend is blocking); `sqlx` is async, so `PgStore`
//! owns a small Tokio runtime and `block_on`s each query. Callers already invoke store methods from
//! `spawn_blocking`, so this never blocks the API's async workers.
//!
//! Phase 5a part 1: the **core data plane** (events, projects, API keys, limits, prices, scores) is
//! implemented and verified against Postgres. Benchmark/dataset/rubric/job methods are stubbed with a
//! clear error and ported in part 2.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::Row;
use tokio::runtime::Runtime;

use lighttrack_core::{
    ApiKey, Benchmark, BenchmarkRun, Dataset, DatasetItem, Job, LimitRule, LlmEvent, ModelPriceRow,
    Operation, Project, Provider, Redaction, Rubric, Score, Status, TokenUsage,
};
use lighttrack_store::{CostRow, Result, Store, StoreError, Usage};

const SCHEMA: &str = include_str!("../../../schema/postgres/001_init.sql");

const EVENT_COLS: &str = "id, project_id, trace_id, span_id, parent_span_id, ts, provider, model, \
    operation, input_tokens, output_tokens, cached_input_tokens, reasoning_tokens, cost_usd, \
    latency_ms, status, error, input, output, tags, source, metadata";

const SCORE_COLS: &str = "id, project_id, event_id, rubric, value, \"max\", pass, reasoning, \
    scored_by, cost_usd, created_at";

const PRICE_COLS: &str = "provider, model, input_per_mtok, output_per_mtok, \
    cached_input_per_mtok, effective_date, source_url";

fn fmt_ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .map_err(|e| StoreError::Other(format!("bad ts {s:?}: {e}")))?
        .with_timezone(&Utc))
}

fn enum_to_str<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_value(v)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| StoreError::Other("enum did not serialize to a string".into()))
}

fn parse_enum<T: DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_value(Value::String(s.to_string())).unwrap_or_default()
}

fn pgerr(e: sqlx::Error) -> StoreError {
    StoreError::Other(format!("postgres: {e}"))
}

fn todo_pg(method: &str) -> StoreError {
    StoreError::Other(format!(
        "postgres backend: `{method}` not yet implemented (Phase 5a part 2)"
    ))
}

/// Postgres-backed [`Store`].
pub struct PgStore {
    pool: PgPool,
    rt: Runtime,
}

impl PgStore {
    /// Connect (sslmode=prefer by default: TLS for cloud, plaintext fallback for local Docker) and
    /// ensure the schema exists.
    pub fn connect(database_url: &str) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| StoreError::Other(format!("tokio runtime: {e}")))?;
        let pool = rt
            .block_on(async {
                PgPoolOptions::new()
                    .max_connections(5)
                    .connect(database_url)
                    .await
            })
            .map_err(pgerr)?;
        let store = Self { pool, rt };
        store.init_schema()?;
        Ok(store)
    }
}

impl Store for PgStore {
    fn init_schema(&self) -> Result<()> {
        self.rt
            .block_on(async { sqlx::raw_sql(SCHEMA).execute(&self.pool).await })
            .map_err(pgerr)?;
        Ok(())
    }

    fn insert_event(&self, ev: &LlmEvent) -> Result<()> {
        let tags = serde_json::to_string(&ev.tags)?;
        let metadata = if ev.metadata.is_null() {
            None
        } else {
            Some(serde_json::to_string(&ev.metadata)?)
        };
        let input = match &ev.input {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        let output = match &ev.output {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO events (id, project_id, trace_id, span_id, parent_span_id, ts, \
                     provider, model, operation, input_tokens, output_tokens, cached_input_tokens, \
                     reasoning_tokens, cost_usd, latency_ms, status, error, input, output, tags, \
                     source, metadata) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)",
                )
                .bind(ev.id.clone())
                .bind(ev.project_id.clone())
                .bind(ev.trace_id.clone())
                .bind(ev.span_id.clone())
                .bind(ev.parent_span_id.clone())
                .bind(fmt_ts(ev.ts))
                .bind(ev.provider.as_str())
                .bind(ev.model.clone())
                .bind(ev.operation.as_str())
                .bind(ev.usage.input as i64)
                .bind(ev.usage.output as i64)
                .bind(ev.usage.cached_input.map(|v| v as i64))
                .bind(ev.usage.reasoning.map(|v| v as i64))
                .bind(ev.cost_usd)
                .bind(ev.latency_ms.map(|v| v as i64))
                .bind(ev.status.as_str())
                .bind(ev.error.clone())
                .bind(input)
                .bind(output)
                .bind(tags)
                .bind(ev.source.clone())
                .bind(metadata)
                .execute(&self.pool)
                .await
            })
            .map_err(pgerr)?;
        Ok(())
    }

    fn list_events(&self, project: Option<&str>, limit: usize) -> Result<Vec<LlmEvent>> {
        let rows = self
            .rt
            .block_on(async {
                match project {
                    Some(p) => {
                        sqlx::query(&format!(
                            "SELECT {EVENT_COLS} FROM events WHERE project_id = $1 ORDER BY ts DESC LIMIT $2"
                        ))
                        .bind(p.to_string())
                        .bind(limit as i64)
                        .fetch_all(&self.pool)
                        .await
                    }
                    None => {
                        sqlx::query(&format!("SELECT {EVENT_COLS} FROM events ORDER BY ts DESC LIMIT $1"))
                            .bind(limit as i64)
                            .fetch_all(&self.pool)
                            .await
                    }
                }
            })
            .map_err(pgerr)?;
        rows.iter().map(event_from_row).collect()
    }

    fn cost_summary(&self, project: Option<&str>) -> Result<Vec<CostRow>> {
        let cols = "project_id, provider, model, COUNT(*) AS calls, \
            COALESCE(SUM(input_tokens),0)::bigint AS it, COALESCE(SUM(output_tokens),0)::bigint AS ot, \
            COALESCE(SUM(cost_usd),0.0) AS cost";
        let rows = self
            .rt
            .block_on(async {
                match project {
                    Some(p) => {
                        sqlx::query(&format!(
                            "SELECT {cols} FROM events WHERE project_id = $1 \
                             GROUP BY project_id, provider, model ORDER BY cost DESC"
                        ))
                        .bind(p.to_string())
                        .fetch_all(&self.pool)
                        .await
                    }
                    None => {
                        sqlx::query(&format!(
                            "SELECT {cols} FROM events GROUP BY project_id, provider, model ORDER BY cost DESC"
                        ))
                        .fetch_all(&self.pool)
                        .await
                    }
                }
            })
            .map_err(pgerr)?;
        rows.iter()
            .map(|row| {
                Ok(CostRow {
                    project_id: row.try_get(0).map_err(pgerr)?,
                    provider: row.try_get(1).map_err(pgerr)?,
                    model: row.try_get(2).map_err(pgerr)?,
                    calls: row.try_get(3).map_err(pgerr)?,
                    input_tokens: row.try_get(4).map_err(pgerr)?,
                    output_tokens: row.try_get(5).map_err(pgerr)?,
                    cost_usd: row.try_get(6).map_err(pgerr)?,
                })
            })
            .collect()
    }

    fn usage_since(&self, project: &str, since: DateTime<Utc>) -> Result<Usage> {
        let row = self
            .rt
            .block_on(async {
                sqlx::query(
                    "SELECT COALESCE(SUM(cost_usd),0.0), COUNT(*), \
                     COALESCE(SUM(input_tokens + output_tokens),0)::bigint \
                     FROM events WHERE project_id = $1 AND ts >= $2",
                )
                .bind(project.to_string())
                .bind(fmt_ts(since))
                .fetch_one(&self.pool)
                .await
            })
            .map_err(pgerr)?;
        Ok(Usage {
            cost_usd: row.try_get(0).map_err(pgerr)?,
            calls: row.try_get(1).map_err(pgerr)?,
            tokens: row.try_get(2).map_err(pgerr)?,
        })
    }

    fn create_project(&self, p: &Project) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO projects (id, name, enabled, redaction, created_at) VALUES ($1,$2,$3,$4,$5)",
                )
                .bind(p.id.clone())
                .bind(p.name.clone())
                .bind(p.enabled as i64)
                .bind(enum_to_str(&p.redaction)?)
                .bind(fmt_ts(p.created_at))
                .execute(&self.pool)
                .await
                .map_err(pgerr)
            })?;
        Ok(())
    }

    fn get_project(&self, id: &str) -> Result<Option<Project>> {
        let row = self
            .rt
            .block_on(async {
                sqlx::query("SELECT id, name, enabled, redaction, created_at FROM projects WHERE id = $1")
                    .bind(id.to_string())
                    .fetch_optional(&self.pool)
                    .await
            })
            .map_err(pgerr)?;
        row.as_ref().map(project_from_row).transpose()
    }

    fn list_projects(&self) -> Result<Vec<Project>> {
        let rows = self
            .rt
            .block_on(async {
                sqlx::query("SELECT id, name, enabled, redaction, created_at FROM projects ORDER BY created_at DESC")
                    .fetch_all(&self.pool)
                    .await
            })
            .map_err(pgerr)?;
        rows.iter().map(project_from_row).collect()
    }

    fn create_api_key(&self, k: &ApiKey) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO api_keys (id, project_id, name, prefix, key_hash, created_at, last_used_at, revoked) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(k.id.clone())
                .bind(k.project_id.clone())
                .bind(k.name.clone())
                .bind(k.prefix.clone())
                .bind(k.key_hash.clone())
                .bind(fmt_ts(k.created_at))
                .bind(k.last_used_at.map(fmt_ts))
                .bind(k.revoked as i64)
                .execute(&self.pool)
                .await
            })
            .map_err(pgerr)?;
        Ok(())
    }

    fn find_api_key_by_prefix(&self, prefix: &str) -> Result<Option<ApiKey>> {
        let row = self
            .rt
            .block_on(async {
                sqlx::query(
                    "SELECT id, project_id, name, prefix, key_hash, created_at, last_used_at, revoked \
                     FROM api_keys WHERE prefix = $1",
                )
                .bind(prefix.to_string())
                .fetch_optional(&self.pool)
                .await
            })
            .map_err(pgerr)?;
        row.as_ref().map(api_key_from_row).transpose()
    }

    fn touch_api_key(&self, id: &str, when: DateTime<Utc>) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("UPDATE api_keys SET last_used_at = $2 WHERE id = $1")
                    .bind(id.to_string())
                    .bind(fmt_ts(when))
                    .execute(&self.pool)
                    .await
            })
            .map_err(pgerr)?;
        Ok(())
    }

    fn create_limit_rule(&self, r: &LimitRule) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO limit_rules (id, project_id, metric, \"window\", threshold, action, enabled) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7)",
                )
                .bind(r.id.clone())
                .bind(r.project_id.clone())
                .bind(enum_to_str(&r.metric)?)
                .bind(enum_to_str(&r.window)?)
                .bind(r.threshold)
                .bind(enum_to_str(&r.action)?)
                .bind(r.enabled as i64)
                .execute(&self.pool)
                .await
                .map_err(pgerr)
            })?;
        Ok(())
    }

    fn list_limit_rules(&self, project: &str, only_enabled: bool) -> Result<Vec<LimitRule>> {
        let sql = if only_enabled {
            "SELECT id, project_id, metric, \"window\", threshold, action, enabled \
             FROM limit_rules WHERE project_id = $1 AND enabled = 1"
        } else {
            "SELECT id, project_id, metric, \"window\", threshold, action, enabled \
             FROM limit_rules WHERE project_id = $1"
        };
        let rows = self
            .rt
            .block_on(async { sqlx::query(sql).bind(project.to_string()).fetch_all(&self.pool).await })
            .map_err(pgerr)?;
        rows.iter().map(limit_rule_from_row).collect()
    }

    fn get_event(&self, id: &str) -> Result<Option<LlmEvent>> {
        let row = self
            .rt
            .block_on(async {
                sqlx::query(&format!("SELECT {EVENT_COLS} FROM events WHERE id = $1"))
                    .bind(id.to_string())
                    .fetch_optional(&self.pool)
                    .await
            })
            .map_err(pgerr)?;
        match row {
            Some(r) => Ok(Some(event_from_row(&r)?)),
            None => Ok(None),
        }
    }

    fn insert_score(&self, s: &Score) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO scores (id, project_id, event_id, rubric, value, \"max\", pass, \
                     reasoning, scored_by, cost_usd, created_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
                )
                .bind(s.id.clone())
                .bind(s.project_id.clone())
                .bind(s.event_id.clone())
                .bind(s.rubric.clone())
                .bind(s.value)
                .bind(s.max)
                .bind(s.pass.map(|b| b as i64))
                .bind(s.reasoning.clone())
                .bind(s.scored_by.clone())
                .bind(s.cost_usd)
                .bind(fmt_ts(s.created_at))
                .execute(&self.pool)
                .await
            })
            .map_err(pgerr)?;
        Ok(())
    }

    fn list_scores(&self, project: Option<&str>, limit: usize) -> Result<Vec<Score>> {
        let rows = self
            .rt
            .block_on(async {
                match project {
                    Some(p) => {
                        sqlx::query(&format!(
                            "SELECT {SCORE_COLS} FROM scores WHERE project_id = $1 ORDER BY created_at DESC LIMIT $2"
                        ))
                        .bind(p.to_string())
                        .bind(limit as i64)
                        .fetch_all(&self.pool)
                        .await
                    }
                    None => {
                        sqlx::query(&format!("SELECT {SCORE_COLS} FROM scores ORDER BY created_at DESC LIMIT $1"))
                            .bind(limit as i64)
                            .fetch_all(&self.pool)
                            .await
                    }
                }
            })
            .map_err(pgerr)?;
        rows.iter().map(score_from_row).collect()
    }

    fn upsert_price(&self, p: &ModelPriceRow) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO model_prices (provider, model, input_per_mtok, output_per_mtok, \
                     cached_input_per_mtok, effective_date, source_url) VALUES ($1,$2,$3,$4,$5,$6,$7) \
                     ON CONFLICT (provider, model) DO UPDATE SET \
                       input_per_mtok = EXCLUDED.input_per_mtok, output_per_mtok = EXCLUDED.output_per_mtok, \
                       cached_input_per_mtok = EXCLUDED.cached_input_per_mtok, \
                       effective_date = EXCLUDED.effective_date, source_url = EXCLUDED.source_url",
                )
                .bind(p.provider.clone())
                .bind(p.model.clone())
                .bind(p.input_per_mtok)
                .bind(p.output_per_mtok)
                .bind(p.cached_input_per_mtok)
                .bind(fmt_ts(p.effective_date))
                .bind(p.source_url.clone())
                .execute(&self.pool)
                .await
            })
            .map_err(pgerr)?;
        Ok(())
    }

    fn list_prices(&self) -> Result<Vec<ModelPriceRow>> {
        let rows = self
            .rt
            .block_on(async {
                sqlx::query(&format!("SELECT {PRICE_COLS} FROM model_prices ORDER BY provider, model"))
                    .fetch_all(&self.pool)
                    .await
            })
            .map_err(pgerr)?;
        rows.iter().map(price_from_row).collect()
    }

    // ---- Phase 5a part 2: not yet ported to Postgres -----------------------
    fn create_benchmark(&self, _b: &Benchmark) -> Result<()> {
        Err(todo_pg("create_benchmark"))
    }
    fn get_benchmark(&self, _id: &str) -> Result<Option<Benchmark>> {
        Err(todo_pg("get_benchmark"))
    }
    fn list_benchmarks(&self, _project: &str) -> Result<Vec<Benchmark>> {
        Err(todo_pg("list_benchmarks"))
    }
    fn create_benchmark_run(&self, _r: &BenchmarkRun) -> Result<()> {
        Err(todo_pg("create_benchmark_run"))
    }
    fn list_benchmark_runs(&self, _benchmark_id: &str) -> Result<Vec<BenchmarkRun>> {
        Err(todo_pg("list_benchmark_runs"))
    }
    fn create_dataset(&self, _d: &Dataset) -> Result<()> {
        Err(todo_pg("create_dataset"))
    }
    fn get_dataset(&self, _id: &str) -> Result<Option<Dataset>> {
        Err(todo_pg("get_dataset"))
    }
    fn list_datasets(&self, _project: &str) -> Result<Vec<Dataset>> {
        Err(todo_pg("list_datasets"))
    }
    fn set_dataset_frozen(&self, _id: &str, _frozen: bool) -> Result<()> {
        Err(todo_pg("set_dataset_frozen"))
    }
    fn create_dataset_item(&self, _item: &DatasetItem) -> Result<()> {
        Err(todo_pg("create_dataset_item"))
    }
    fn list_dataset_items(&self, _dataset_id: &str) -> Result<Vec<DatasetItem>> {
        Err(todo_pg("list_dataset_items"))
    }
    fn create_rubric(&self, _r: &Rubric) -> Result<()> {
        Err(todo_pg("create_rubric"))
    }
    fn get_rubric(&self, _id: &str) -> Result<Option<Rubric>> {
        Err(todo_pg("get_rubric"))
    }
    fn list_rubrics(&self, _project: &str) -> Result<Vec<Rubric>> {
        Err(todo_pg("list_rubrics"))
    }
    fn create_job(&self, _j: &Job) -> Result<()> {
        Err(todo_pg("create_job"))
    }
    fn claim_job(&self, _stale_before: DateTime<Utc>) -> Result<Option<Job>> {
        Err(todo_pg("claim_job"))
    }
    fn update_job_progress(&self, _id: &str, _progress: &str) -> Result<()> {
        Err(todo_pg("update_job_progress"))
    }
    fn finish_job(&self, _id: &str, _status: &str, _result: &Value, _error: Option<&str>) -> Result<()> {
        Err(todo_pg("finish_job"))
    }
    fn get_job(&self, _id: &str) -> Result<Option<Job>> {
        Err(todo_pg("get_job"))
    }
    fn list_jobs(&self, _status: Option<&str>, _limit: usize) -> Result<Vec<Job>> {
        Err(todo_pg("list_jobs"))
    }
}

// --- row converters ---------------------------------------------------------

fn event_from_row(row: &PgRow) -> Result<LlmEvent> {
    let ts: String = row.try_get(5).map_err(pgerr)?;
    let provider: String = row.try_get(6).map_err(pgerr)?;
    let operation: String = row.try_get(8).map_err(pgerr)?;
    let status: String = row.try_get(15).map_err(pgerr)?;
    let input: Option<String> = row.try_get(17).map_err(pgerr)?;
    let output: Option<String> = row.try_get(18).map_err(pgerr)?;
    let tags: Option<String> = row.try_get(19).map_err(pgerr)?;
    let metadata: Option<String> = row.try_get(21).map_err(pgerr)?;

    Ok(LlmEvent {
        id: row.try_get(0).map_err(pgerr)?,
        project_id: row.try_get(1).map_err(pgerr)?,
        trace_id: row.try_get(2).map_err(pgerr)?,
        span_id: row.try_get(3).map_err(pgerr)?,
        parent_span_id: row.try_get(4).map_err(pgerr)?,
        ts: parse_ts(&ts)?,
        provider: parse_enum::<Provider>(&provider),
        model: row.try_get(7).map_err(pgerr)?,
        operation: parse_enum::<Operation>(&operation),
        usage: TokenUsage {
            input: row.try_get::<i64, _>(9).map_err(pgerr)? as u64,
            output: row.try_get::<i64, _>(10).map_err(pgerr)? as u64,
            cached_input: row.try_get::<Option<i64>, _>(11).map_err(pgerr)?.map(|v| v as u64),
            reasoning: row.try_get::<Option<i64>, _>(12).map_err(pgerr)?.map(|v| v as u64),
        },
        cost_usd: row.try_get(13).map_err(pgerr)?,
        latency_ms: row.try_get::<Option<i64>, _>(14).map_err(pgerr)?.map(|v| v as u64),
        status: parse_enum::<Status>(&status),
        error: row.try_get(16).map_err(pgerr)?,
        input: match input {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        },
        output: match output {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        },
        tags: match tags {
            Some(s) => serde_json::from_str(&s)?,
            None => Vec::new(),
        },
        source: row.try_get(20).map_err(pgerr)?,
        metadata: match metadata {
            Some(s) => serde_json::from_str(&s)?,
            None => Value::Null,
        },
    })
}

fn project_from_row(row: &PgRow) -> Result<Project> {
    let redaction: String = row.try_get(3).map_err(pgerr)?;
    let created_at: String = row.try_get(4).map_err(pgerr)?;
    Ok(Project {
        id: row.try_get(0).map_err(pgerr)?,
        name: row.try_get(1).map_err(pgerr)?,
        enabled: row.try_get::<i64, _>(2).map_err(pgerr)? != 0,
        redaction: parse_enum::<Redaction>(&redaction),
        created_at: parse_ts(&created_at)?,
    })
}

fn api_key_from_row(row: &PgRow) -> Result<ApiKey> {
    let created_at: String = row.try_get(5).map_err(pgerr)?;
    let last_used: Option<String> = row.try_get(6).map_err(pgerr)?;
    Ok(ApiKey {
        id: row.try_get(0).map_err(pgerr)?,
        project_id: row.try_get(1).map_err(pgerr)?,
        name: row.try_get(2).map_err(pgerr)?,
        prefix: row.try_get(3).map_err(pgerr)?,
        key_hash: row.try_get(4).map_err(pgerr)?,
        created_at: parse_ts(&created_at)?,
        last_used_at: match last_used {
            Some(s) => Some(parse_ts(&s)?),
            None => None,
        },
        revoked: row.try_get::<i64, _>(7).map_err(pgerr)? != 0,
    })
}

fn limit_rule_from_row(row: &PgRow) -> Result<LimitRule> {
    let metric: String = row.try_get(2).map_err(pgerr)?;
    let window: String = row.try_get(3).map_err(pgerr)?;
    let action: String = row.try_get(5).map_err(pgerr)?;
    Ok(LimitRule {
        id: row.try_get(0).map_err(pgerr)?,
        project_id: row.try_get(1).map_err(pgerr)?,
        metric: parse_enum(&metric),
        window: parse_enum(&window),
        threshold: row.try_get(4).map_err(pgerr)?,
        action: parse_enum(&action),
        enabled: row.try_get::<i64, _>(6).map_err(pgerr)? != 0,
    })
}

fn score_from_row(row: &PgRow) -> Result<Score> {
    let created_at: String = row.try_get(10).map_err(pgerr)?;
    Ok(Score {
        id: row.try_get(0).map_err(pgerr)?,
        project_id: row.try_get(1).map_err(pgerr)?,
        event_id: row.try_get(2).map_err(pgerr)?,
        rubric: row.try_get(3).map_err(pgerr)?,
        value: row.try_get(4).map_err(pgerr)?,
        max: row.try_get(5).map_err(pgerr)?,
        pass: row.try_get::<Option<i64>, _>(6).map_err(pgerr)?.map(|v| v != 0),
        reasoning: row.try_get(7).map_err(pgerr)?,
        scored_by: row.try_get(8).map_err(pgerr)?,
        cost_usd: row.try_get(9).map_err(pgerr)?,
        created_at: parse_ts(&created_at)?,
    })
}

fn price_from_row(row: &PgRow) -> Result<ModelPriceRow> {
    let effective_date: String = row.try_get(5).map_err(pgerr)?;
    Ok(ModelPriceRow {
        provider: row.try_get(0).map_err(pgerr)?,
        model: row.try_get(1).map_err(pgerr)?,
        input_per_mtok: row.try_get(2).map_err(pgerr)?,
        output_per_mtok: row.try_get(3).map_err(pgerr)?,
        cached_input_per_mtok: row.try_get(4).map_err(pgerr)?,
        effective_date: parse_ts(&effective_date)?,
        source_url: row.try_get(6).map_err(pgerr)?,
    })
}
