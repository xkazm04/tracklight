# LightTrack — Roadmap

Evolved daily. Checked items are done; the rest is the plan we agreed on.

## Phase 0 — Scaffold ✅ (today)
- [x] Repo, workspace, docs, decisions log
- [x] `core`: normalized `LlmEvent`, `PriceBook` + cost calc, `LimitRule` eval, scoring/benchmark types
- [x] SQLite + BigQuery DDL, `pricing.json`
- [x] `cargo build` green for the whole workspace

## Phase 1 — Ingest → query (local, SQLite) ✅
- [x] `Store` trait + SQLite backend (`rusqlite`, bundled)
- [x] `api`: `POST /v1/events` (normalize + compute cost + write), `GET /v1/events`, `GET /v1/costs`
- [x] Verify: synthetic traffic from 3 fake "apps", cost rollups confirmed against the running server
- [x] Project + API-key model; `dev` (relaxed) vs `enforced` auth  → done in Phase 2
- [ ] Minimal client snippet (Rust + Python) to wrap OpenAI/Anthropic/Gemini calls  → Phase 2.5

## Phase 2 — Projects, keys, limits ✅
- [x] CRUD for projects, API keys (salted-hash), limit rules (via `lt` CLI + API)
- [x] Rolling-window usage + limit evaluation on ingest (cost/calls/tokens × hour/day/month)
- [x] `GET /v1/limits/status` advisory throttle flag; breaches surfaced in the ingest response
- [x] `dev` vs `enforced` auth (admin key + per-project keys); verified 401/403 boundaries
- [x] `lt` CLI (projects/keys/limits/costs/events) — verified against the enforced server
- [~] Inline breach alerts: server-side `[ALERT]` log done; webhook/ntfy/Pub/Sub delivery deferred to Phase 5 (cloud)

## Phase 3 — Scoring & benchmarks
- [ ] `runner`: job queue (in-proc channel locally), `claude -p --json-schema` judge, parse verdict + cost
- [ ] Online sampling → scores; `cli`/MCP to trigger
- [ ] `BenchmarkDefinition` + run + scorecard + regression baseline

## Phase 4 — MCP
- [ ] `mcp`: `query_traces`, `get_cost_summary`, `list_projects`, `get_limit_status`, `run_benchmark`
- [ ] Register with Claude Code; dogfood querying LightTrack from the terminal

## Phase 5 — Cloud move
- [ ] BigQuery `Store` backend + Firestore config backend
- [ ] Containerize `api` → Cloud Run; `runner` → e2-micro; Secret Manager for keys
- [ ] Pub/Sub job dispatch; Cloud Scheduler periodic checks → Cloud Function alerts
- [ ] Looker Studio dashboard on BigQuery
- [ ] Enforce API-key auth + TLS

## Parallelism & scale targets
- Expected: 5–10 apps × 10–100 calls/hour ≈ ≤1k calls/hr. `api` handles ingest concurrently (async axum);
  batched writes to the Store. Comfortably inside every GCP free-tier ceiling.
