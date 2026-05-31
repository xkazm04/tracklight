//! SQLite-backed [`Store`] — the local-development backend (bundled SQLite, no external service).
//!
//! `SqliteStore` holds a mutex-guarded connection; the `Store` impl locks it and delegates to a
//! per-domain submodule of free functions over `&Connection` (`events`, `scores`, `projects`,
//! `benchmarks`, `datasets`, `rubrics`, `prices`, `jobs`). Shared helpers live in `util`.

mod benchmarks;
mod datasets;
mod events;
mod jobs;
mod prices;
mod projects;
mod rubrics;
mod scores;
mod util;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::Value;

use lighttrack_core::{
    ApiKey, Benchmark, BenchmarkRun, Dataset, DatasetItem, Job, LimitRule, LlmEvent, ModelPriceRow,
    Project, Rubric, Score,
};

use crate::{CostRow, Result, Store, Usage};

const SCHEMA: &str = include_str!("../../../../schema/sqlite/001_init.sql");

/// SQLite store. A single connection guarded by a mutex — fine for our throughput (≤1k calls/hr).
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// Open (creating parent dirs and the file if needed) and ensure the schema exists.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let store = Self {
            conn: Mutex::new(Connection::open(path)?),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// In-memory store, for tests.
    pub fn open_in_memory() -> Result<Self> {
        let store = Self {
            conn: Mutex::new(Connection::open_in_memory()?),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Run a closure with the locked connection.
    fn with<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        f(&self.conn.lock().unwrap())
    }
}

impl Store for SqliteStore {
    fn init_schema(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch(SCHEMA)?;
            Ok(())
        })
    }

    // --- events ---
    fn insert_event(&self, ev: &LlmEvent) -> Result<()> {
        self.with(|c| events::insert(c, ev))
    }
    fn list_events(&self, project: Option<&str>, limit: usize) -> Result<Vec<LlmEvent>> {
        self.with(|c| events::list(c, project, limit))
    }
    fn cost_summary(&self, project: Option<&str>) -> Result<Vec<CostRow>> {
        self.with(|c| events::cost_summary(c, project))
    }
    fn usage_since(&self, project: &str, since: DateTime<Utc>) -> Result<Usage> {
        self.with(|c| events::usage_since(c, project, since))
    }
    fn get_event(&self, id: &str) -> Result<Option<LlmEvent>> {
        self.with(|c| events::get(c, id))
    }

    // --- scores ---
    fn insert_score(&self, s: &Score) -> Result<()> {
        self.with(|c| scores::insert(c, s))
    }
    fn list_scores(&self, project: Option<&str>, limit: usize) -> Result<Vec<Score>> {
        self.with(|c| scores::list(c, project, limit))
    }

    // --- projects / api keys / limits ---
    fn create_project(&self, p: &Project) -> Result<()> {
        self.with(|c| projects::create(c, p))
    }
    fn get_project(&self, id: &str) -> Result<Option<Project>> {
        self.with(|c| projects::get(c, id))
    }
    fn list_projects(&self) -> Result<Vec<Project>> {
        self.with(projects::list)
    }
    fn create_api_key(&self, k: &ApiKey) -> Result<()> {
        self.with(|c| projects::create_key(c, k))
    }
    fn find_api_key_by_prefix(&self, prefix: &str) -> Result<Option<ApiKey>> {
        self.with(|c| projects::find_key_by_prefix(c, prefix))
    }
    fn touch_api_key(&self, id: &str, when: DateTime<Utc>) -> Result<()> {
        self.with(|c| projects::touch_key(c, id, when))
    }
    fn create_limit_rule(&self, r: &LimitRule) -> Result<()> {
        self.with(|c| projects::create_limit(c, r))
    }
    fn list_limit_rules(&self, project: &str, only_enabled: bool) -> Result<Vec<LimitRule>> {
        self.with(|c| projects::list_limits(c, project, only_enabled))
    }

    // --- benchmarks ---
    fn create_benchmark(&self, b: &Benchmark) -> Result<()> {
        self.with(|c| benchmarks::create(c, b))
    }
    fn get_benchmark(&self, id: &str) -> Result<Option<Benchmark>> {
        self.with(|c| benchmarks::get(c, id))
    }
    fn list_benchmarks(&self, project: &str) -> Result<Vec<Benchmark>> {
        self.with(|c| benchmarks::list(c, project))
    }
    fn create_benchmark_run(&self, r: &BenchmarkRun) -> Result<()> {
        self.with(|c| benchmarks::create_run(c, r))
    }
    fn list_benchmark_runs(&self, benchmark_id: &str) -> Result<Vec<BenchmarkRun>> {
        self.with(|c| benchmarks::list_runs(c, benchmark_id))
    }

    // --- prices ---
    fn upsert_price(&self, p: &ModelPriceRow) -> Result<()> {
        self.with(|c| prices::upsert(c, p))
    }
    fn list_prices(&self) -> Result<Vec<ModelPriceRow>> {
        self.with(prices::list)
    }

    // --- datasets ---
    fn create_dataset(&self, d: &Dataset) -> Result<()> {
        self.with(|c| datasets::create(c, d))
    }
    fn get_dataset(&self, id: &str) -> Result<Option<Dataset>> {
        self.with(|c| datasets::get(c, id))
    }
    fn list_datasets(&self, project: &str) -> Result<Vec<Dataset>> {
        self.with(|c| datasets::list(c, project))
    }
    fn set_dataset_frozen(&self, id: &str, frozen: bool) -> Result<()> {
        self.with(|c| datasets::set_frozen(c, id, frozen))
    }
    fn create_dataset_item(&self, item: &DatasetItem) -> Result<()> {
        self.with(|c| datasets::create_item(c, item))
    }
    fn list_dataset_items(&self, dataset_id: &str) -> Result<Vec<DatasetItem>> {
        self.with(|c| datasets::list_items(c, dataset_id))
    }

    // --- rubrics ---
    fn create_rubric(&self, r: &Rubric) -> Result<()> {
        self.with(|c| rubrics::create(c, r))
    }
    fn get_rubric(&self, id: &str) -> Result<Option<Rubric>> {
        self.with(|c| rubrics::get(c, id))
    }
    fn list_rubrics(&self, project: &str) -> Result<Vec<Rubric>> {
        self.with(|c| rubrics::list(c, project))
    }

    // --- jobs ---
    fn create_job(&self, j: &Job) -> Result<()> {
        self.with(|c| jobs::create(c, j))
    }
    fn claim_job(&self, stale_before: DateTime<Utc>) -> Result<Option<Job>> {
        self.with(|c| jobs::claim(c, stale_before))
    }
    fn update_job_progress(&self, id: &str, progress: &str) -> Result<()> {
        self.with(|c| jobs::update_progress(c, id, progress))
    }
    fn finish_job(&self, id: &str, status: &str, result: &Value, error: Option<&str>) -> Result<()> {
        self.with(|c| jobs::finish(c, id, status, result, error))
    }
    fn get_job(&self, id: &str) -> Result<Option<Job>> {
        self.with(|c| jobs::get(c, id))
    }
    fn list_jobs(&self, status: Option<&str>, limit: usize) -> Result<Vec<Job>> {
        self.with(|c| jobs::list(c, status, limit))
    }
}
