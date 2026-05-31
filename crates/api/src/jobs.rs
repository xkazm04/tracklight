//! Job queue (Phase 3.6d) — enqueue returns immediately; `lt-runner serve` executes.

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use lighttrack_core::{new_id, Job};

use crate::benchmarks::load_benchmark_authorized;
use crate::error::ApiError;
use crate::guards::{authenticate, ensure_can_admin};
use crate::state::{spawn_db, AppState};

#[derive(Deserialize)]
pub(crate) struct EnqueueReq {
    #[serde(default = "default_samples")]
    samples: u32,
    #[serde(default)]
    heal: bool,
}

fn default_samples() -> u32 {
    1
}

pub(crate) async fn enqueue_benchmark(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<EnqueueReq>,
) -> Result<Json<Job>, ApiError> {
    let p = authenticate(&st, &headers).await?;
    ensure_can_admin(&p)?;
    let bench = load_benchmark_authorized(&st, &p, &id).await?;
    let job = Job {
        id: new_id(),
        job_type: "bench_run".to_string(),
        payload: serde_json::json!({ "benchmark_id": bench.id, "samples": req.samples, "heal": req.heal }),
        status: "queued".to_string(),
        attempts: 0,
        max_attempts: 3,
        progress: None,
        error: None,
        result: serde_json::Value::Null,
        claimed_at: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let store = st.store.clone();
    let j2 = job.clone();
    spawn_db(move || store.create_job(&j2)).await?;
    Ok(Json(job))
}

#[derive(Deserialize)]
pub(crate) struct JobsParams {
    status: Option<String>,
    limit: Option<usize>,
}

pub(crate) async fn list_jobs(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<JobsParams>,
) -> Result<Json<Vec<Job>>, ApiError> {
    ensure_can_admin(&authenticate(&st, &headers).await?)?;
    let store = st.store.clone();
    let status = q.status;
    let limit = q.limit.unwrap_or(50).min(1000);
    let jobs = spawn_db(move || store.list_jobs(status.as_deref(), limit)).await?;
    Ok(Json(jobs))
}

pub(crate) async fn get_job(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Job>, ApiError> {
    ensure_can_admin(&authenticate(&st, &headers).await?)?;
    let store = st.store.clone();
    let id2 = id.clone();
    let job = spawn_db(move || store.get_job(&id2))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("job '{id}' not found")))?;
    Ok(Json(job))
}

#[derive(Deserialize)]
pub(crate) struct ClaimReq {
    #[serde(default = "default_stale_secs")]
    stale_secs: i64,
}

fn default_stale_secs() -> i64 {
    600
}

pub(crate) async fn claim_job(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ClaimReq>,
) -> Result<Json<Option<Job>>, ApiError> {
    ensure_can_admin(&authenticate(&st, &headers).await?)?;
    let stale_before = Utc::now() - chrono::Duration::seconds(req.stale_secs.max(0));
    let store = st.store.clone();
    let job = spawn_db(move || store.claim_job(stale_before)).await?;
    Ok(Json(job))
}

#[derive(Deserialize)]
pub(crate) struct ProgressReq {
    progress: String,
}

pub(crate) async fn job_progress(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ProgressReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_can_admin(&authenticate(&st, &headers).await?)?;
    let store = st.store.clone();
    spawn_db(move || store.update_job_progress(&id, &req.progress)).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub(crate) struct FinishReq {
    status: String,
    #[serde(default)]
    result: serde_json::Value,
    #[serde(default)]
    error: Option<String>,
}

pub(crate) async fn job_finish(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<FinishReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_can_admin(&authenticate(&st, &headers).await?)?;
    let store = st.store.clone();
    spawn_db(move || store.finish_job(&id, &req.status, &req.result, req.error.as_deref())).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
