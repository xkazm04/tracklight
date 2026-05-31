//! Benchmarks (Phase 3.5) — definitions, runs, and the comparison target matrix.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use lighttrack_core::{
    new_id, BenchTarget, Benchmark, BenchmarkCase, BenchmarkRun,
};

use crate::auth::Principal;
use crate::error::ApiError;
use crate::guards::{authenticate, ensure_can_admin, resolve_read_project};
use crate::state::{spawn_db, AppState};

#[derive(Deserialize)]
pub(crate) struct CreateBenchmarkReq {
    name: String,
    /// Freeform rubric text (single-score mode); optional when `rubric_id` is set.
    #[serde(default)]
    rubric: String,
    #[serde(default = "default_judge_model")]
    judge_model: String,
    #[serde(default)]
    target: serde_json::Value,
    /// Comparison matrix: generate candidate outputs from each of these targets (Phase 3.6e).
    #[serde(default)]
    targets: Vec<BenchTarget>,
    #[serde(default)]
    dataset: Vec<BenchmarkCase>,
    /// Reference a stored dataset by id instead of (or in addition to) an inline dataset.
    #[serde(default)]
    dataset_ref: Option<String>,
    /// Optional structured rubric (id) for per-dimension judging.
    #[serde(default)]
    rubric_id: Option<String>,
    #[serde(default)]
    baseline_score: Option<f64>,
}

fn default_judge_model() -> String {
    "haiku".to_string()
}

pub(crate) async fn create_benchmark(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
    Json(req): Json<CreateBenchmarkReq>,
) -> Result<Json<Benchmark>, ApiError> {
    ensure_can_admin(&authenticate(&st, &headers).await?)?;
    let b = Benchmark {
        id: new_id(),
        project_id: pid,
        name: req.name,
        rubric: req.rubric,
        judge_model: req.judge_model,
        // The target matrix (if any) is stored in the `target` field as a JSON array.
        target: if req.targets.is_empty() {
            req.target
        } else {
            serde_json::to_value(&req.targets).unwrap_or(serde_json::Value::Null)
        },
        dataset_ref: req.dataset_ref,
        dataset: req.dataset,
        rubric_id: req.rubric_id,
        baseline_score: req.baseline_score,
        created_at: Utc::now(),
    };
    let store = st.store.clone();
    let b2 = b.clone();
    spawn_db(move || store.create_benchmark(&b2)).await?;
    Ok(Json(b))
}

pub(crate) async fn list_benchmarks(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
) -> Result<Json<Vec<Benchmark>>, ApiError> {
    let p = authenticate(&st, &headers).await?;
    resolve_read_project(&p, Some(&pid))?;
    let store = st.store.clone();
    let v = spawn_db(move || store.list_benchmarks(&pid)).await?;
    Ok(Json(v))
}

/// Fetch a benchmark and authorize project-key access to it.
pub(crate) async fn load_benchmark_authorized(
    st: &AppState,
    p: &Principal,
    id: &str,
) -> Result<Benchmark, ApiError> {
    let store = st.store.clone();
    let id2 = id.to_string();
    let bench = spawn_db(move || store.get_benchmark(&id2))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("benchmark '{id}' not found")))?;
    if let Principal::Project(pid) = p {
        if &bench.project_id != pid {
            return Err(ApiError::forbidden("key not authorized for that benchmark"));
        }
    }
    Ok(bench)
}

pub(crate) async fn get_benchmark(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Benchmark>, ApiError> {
    let p = authenticate(&st, &headers).await?;
    Ok(Json(load_benchmark_authorized(&st, &p, &id).await?))
}

pub(crate) async fn list_benchmark_runs(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<BenchmarkRun>>, ApiError> {
    let p = authenticate(&st, &headers).await?;
    load_benchmark_authorized(&st, &p, &id).await?; // authorize
    let store = st.store.clone();
    let runs = spawn_db(move || store.list_benchmark_runs(&id)).await?;
    Ok(Json(runs))
}

pub(crate) async fn post_benchmark_run(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(run): Json<BenchmarkRun>,
) -> Result<Json<BenchmarkRun>, ApiError> {
    let p = authenticate(&st, &headers).await?;
    load_benchmark_authorized(&st, &p, &run.benchmark_id).await?; // authorize via the benchmark
    let store = st.store.clone();
    let run2 = run.clone();
    spawn_db(move || store.create_benchmark_run(&run2)).await?;
    Ok(Json(run))
}
