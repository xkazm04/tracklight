# LightTrack — Architecture

## 1. Goals & non-goals
**Goals:** headless-first LLM observability for 5–10 apps (~10–100 calls/hour each) across OpenAI /
Gemini / Anthropic; open, queryable data; per-project cost/limit tracking; LLM-as-judge scoring &
benchmarking; near-zero infra cost on GCP free tier; MCP access for Claude Code.

**Non-goals (for now):** multi-org SaaS, fine-grained RBAC, a bespoke web UI (use Looker Studio),
tracking LightTrack's own internal calls.

## 2. Data flow
```
 Monitored apps (local or cloud)                         LightTrack
 ────────────────────────────────                        ──────────
  OpenAI / Gemini / Anthropic SDK call
        │  (1) emit normalized event
        ▼            (thin SDK / HTTP POST / OTel GenAI)
   ┌──────────────────────────────┐   (2) auth(project API key), normalize, compute cost
   │  lighttrack-api  (axum)       │──▶ (3) write event ──▶  Store (SQLite local / BigQuery cloud)
   │  POST /v1/events              │   (4) update rolling counters, evaluate LimitRules
   │  GET  /v1/traces|costs|...    │        └─ breach ──▶ alert (Pub/Sub→Fn→email/Slack/ntfy)
   │  GET  /v1/limits/status       │                       + set advisory throttle flag
   └───────────┬──────────────────┘
               │ (5) enqueue scoring/benchmark jobs (Pub/Sub cloud / channel local)
               ▼
   ┌──────────────────────────────┐   (6) claude -p --output-format json --json-schema <verdict>
   │  lighttrack-runner            │──▶  THE LLM ENGINE (unbudgeted)
   │  pulls jobs, runs judge       │   (7) write Score rows back via API/Store
   └──────────────────────────────┘

   ┌──────────────────────────────┐
   │  lighttrack-mcp               │  read-mostly tools over the Store, for Claude Code / agents
   └──────────────────────────────┘
```

## 3. Components
| Crate | Role | Deploys to |
|---|---|---|
| `core` | Normalized `LlmEvent`, `PriceBook` + cost calc, `LimitRule` eval, scoring/benchmark types, `Store` trait (later). Pure, no I/O. | lib, used everywhere |
| `api` | Ingest + query REST (axum). API-key auth, cost computation, limit evaluation, job enqueue. | local box → **Cloud Run** |
| `runner` | Subscribes to jobs, invokes `claude -p`, parses JSON verdicts, writes scores. The judge. | local box → **e2-micro** |
| `mcp` | MCP server: `query_traces`, `get_cost_summary`, `list_projects`, `get_limit_status`, `run_benchmark`, … | wherever Claude Code runs |
| `cli` | Operator tool: query, manage projects/keys, define & trigger benchmarks. | anywhere |

## 4. Ingestion contract
Apps send a **normalized event** (see `docs/DATA_MODEL.md`). Two front doors, same internal model:
1. **`POST /v1/events`** — simple JSON, the default. A ~30-line client snippet per language wraps each
   provider call (record model, usage, latency, status) and posts the event. Cost is computed server-side.
2. **OTel GenAI** (later) — accept OTLP/HTTP using the GenAI semantic conventions and map spans → events.
   Keeps us vendor-neutral (the anti-lock-in lever vs Langfuse).

Provider SDKs already return token usage; the client just forwards it. Prompts/outputs are **optional**
and **redactable** per project (store nothing, hashes, or full text).

> A future **gateway/proxy mode** (apps route calls *through* LightTrack) would let limits *block* inline
> instead of advising. Deferred — it adds latency and a critical-path dependency.

## 5. Storage — local→cloud parity
A `Store` trait abstracts persistence. Two backends:
- **Local (`v0`): SQLite** via `rusqlite` (bundled) — rock-solid on Windows, zero external services.
- **Cloud: BigQuery** for events/scores (the "do anything with the data" analytical store; 10 GB + 1 TB/mo
  query free) + **Firestore** for hot config (projects, keys, limit rules, counters).

Schemas are kept in lockstep (`schema/sqlite` ↔ `schema/bigquery`) so analytical queries port. DuckDB is a
drop-in local upgrade if we want columnar parity with BigQuery later.

## 6. Cost accounting
`PriceBook` (from `config/pricing.json`, keyed `"<provider>/<model>"`) → `cost_usd(provider, model, usage)`.
Cached-input tokens are billed at the cached rate when present. Events may carry a provider-reported cost
(e.g. Claude Code's `total_cost_usd`); otherwise we compute. Prices in the repo are **approximate — verify
against provider pricing pages** before trusting cost dashboards.

## 7. Limits (incoming traffic trips them; judge is exempt)
`LimitRule { project_id, metric: cost|calls|tokens, window: hour|day|month, threshold, action }`.
On each ingested event we update the project's rolling counter for the window and `evaluate()` matching
rules. Actions: **Alert** (notify), **Throttle** (set an advisory flag readable via `GET /v1/limits/status`
and MCP — cooperating apps self-throttle), **Block** (advisory now; enforceable only in gateway mode).
The scoring/benchmark engine is **not** subject to limits.

## 8. Scoring & benchmarking engine
- **Online scoring:** sample events → enqueue → runner runs a rubric prompt via
  `claude -p --output-format json --json-schema <JudgeVerdict>` → store `Score`.
- **Benchmark:** a `BenchmarkDefinition` (dataset of inputs [+expected], target, rubric, judge model) →
  run target → judge each output → aggregate a scorecard in the Store → track over time → alert on
  regression vs baseline.
- **Engine is pluggable** (`claude -p` → direct API → other provider) and **unbudgeted**. Default judge
  model **Haiku** for cost, escalate to Opus for hard rubrics. The judge's own spend is recorded as a
  `Score.cost_usd` so we can watch Agent-SDK-credit burn — but never throttled.

## 9. Security
- **API keys per project** for ingest (`Authorization: Bearer lt_<prefix>_<secret>`); only a salted hash is
  stored. An **admin key** guards management endpoints.
- **Local dev:** bind to `127.0.0.1`; auth can run in a relaxed `dev` mode.
- **e2-micro:** API keys enforced; TLS via Cloud Run (managed) or Caddy in front of the VM. Secrets live in
  **Secret Manager** (cloud) / a git-ignored `.env`/`*.local.toml` (local), never committed.

## 10. Notifications
Cloud Scheduler (3 free jobs) fires periodic checks (rolling cost, score regression) → Pub/Sub → Cloud
Function → email (SendGrid/Gmail) / Slack webhook / ntfy. Plus inline limit-breach alerts from `api`, and
native GCP budget alerts for infra spend.

## 11. Deployment
- **Phase A (now): local.** `cargo run` for `api` + `runner`; SQLite file; `claude -p` on this machine.
- **Phase B: GCP.** `api`→Cloud Run (container, scales to zero), `runner`→e2-micro (orchestrates remote
  `claude -p`), BigQuery + Firestore, Pub/Sub, Cloud Scheduler, Secret Manager. Looker Studio on BigQuery.

See `docs/ROADMAP.md` for sequencing and `docs/DECISIONS.md` for the rationale behind each choice.
