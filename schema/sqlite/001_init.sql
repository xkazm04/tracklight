-- LightTrack local store (SQLite). Mirrors schema/bigquery/001_init.sql.
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS projects (
  id          TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  enabled     INTEGER NOT NULL DEFAULT 1,
  redaction   TEXT NOT NULL DEFAULT 'none',   -- none | hash | drop
  created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS api_keys (
  id           TEXT PRIMARY KEY,
  project_id   TEXT NOT NULL REFERENCES projects(id),
  name         TEXT NOT NULL,
  prefix       TEXT NOT NULL,
  key_hash     TEXT NOT NULL,
  created_at   TEXT NOT NULL,
  last_used_at TEXT,
  revoked      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_api_keys_prefix ON api_keys(prefix);

CREATE TABLE IF NOT EXISTS events (
  id                  TEXT PRIMARY KEY,
  project_id          TEXT NOT NULL,
  trace_id            TEXT,
  span_id             TEXT,
  parent_span_id      TEXT,
  ts                  TEXT NOT NULL,
  provider            TEXT NOT NULL,
  model               TEXT NOT NULL,
  operation           TEXT NOT NULL DEFAULT 'chat',
  input_tokens        INTEGER NOT NULL DEFAULT 0,
  output_tokens       INTEGER NOT NULL DEFAULT 0,
  cached_input_tokens INTEGER,
  reasoning_tokens    INTEGER,
  cost_usd            REAL,
  latency_ms          INTEGER,
  status              TEXT NOT NULL DEFAULT 'success',
  error               TEXT,
  input               TEXT,        -- JSON
  output              TEXT,        -- JSON
  tags                TEXT,        -- JSON array
  source              TEXT,
  metadata            TEXT         -- JSON
);
CREATE INDEX IF NOT EXISTS idx_events_project_ts ON events(project_id, ts);
CREATE INDEX IF NOT EXISTS idx_events_trace ON events(trace_id);

CREATE TABLE IF NOT EXISTS limit_rules (
  id          TEXT PRIMARY KEY,
  project_id  TEXT NOT NULL,
  metric      TEXT NOT NULL,   -- cost_usd | calls | tokens
  window      TEXT NOT NULL,   -- hour | day | month
  threshold   REAL NOT NULL,
  action      TEXT NOT NULL,   -- alert | throttle | block
  enabled     INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS scores (
  id          TEXT PRIMARY KEY,
  project_id  TEXT NOT NULL,
  event_id    TEXT,
  rubric      TEXT NOT NULL,
  value       REAL NOT NULL,
  max         REAL NOT NULL DEFAULT 1.0,
  pass        INTEGER,
  reasoning   TEXT,
  scored_by   TEXT NOT NULL,
  cost_usd    REAL,
  created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_scores_project ON scores(project_id, created_at);

CREATE TABLE IF NOT EXISTS benchmarks (
  id             TEXT PRIMARY KEY,
  project_id     TEXT NOT NULL,
  name           TEXT NOT NULL,
  rubric         TEXT NOT NULL,
  judge_model    TEXT NOT NULL,
  target         TEXT,         -- JSON
  dataset_ref    TEXT,
  baseline_score REAL,
  created_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS benchmark_runs (
  id           TEXT PRIMARY KEY,
  benchmark_id TEXT NOT NULL REFERENCES benchmarks(id),
  started_at   TEXT NOT NULL,
  finished_at  TEXT,
  n_cases      INTEGER NOT NULL DEFAULT 0,
  mean_score   REAL,
  pass_rate    REAL,
  cost_usd     REAL,
  status       TEXT NOT NULL DEFAULT 'running'
);
