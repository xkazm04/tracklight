---
name: onboard
description: >-
  Guided, chronological onboarding to install, configure, and DEPLOY LightTrack onto the user's
  preferred stack. Walks the user through picking a database (SQLite/DuckDB/libSQL local; Postgres /
  Neon / Supabase / Cloud SQL / RDS / Azure DB; Firestore; BigQuery analytical) and a deploy target
  (local Docker Compose; Cloud Run / App Runner / Container Apps; Kubernetes via Helm), collects the
  required credentials securely, then performs the integration and deployment on the user's behalf and
  verifies it. Use when the user wants to set up, install, configure, onboard, or deploy LightTrack —
  e.g. "set up LightTrack", "deploy to AWS/GCP/Azure", "run it on Postgres", "get me started".
---

# LightTrack onboarding (guided, chronological)

You are setting up LightTrack for a developer on **their** preferred stack, end to end. Drive the flow
with `AskUserQuestion` at each decision point, collect only the credentials their choices require, then
do the integration + deployment yourself and verify it. Be concrete and idempotent — the user may re-run
this. Read `docs/PACKAGING.md` and `docs/ARCHITECTURE.md` for the architecture; `docs/ROADMAP.md` for what
is implemented vs planned.

## Guardrails (always)
- **Secrets never get committed.** Put credentials in a git-ignored `.env` / `*.local.toml`, or a cloud
  secret manager. Confirm `.gitignore` covers them before writing. Never paste a key into a tracked file,
  a commit, or a log line.
- **Interactive logins are the user's to run.** For `gcloud auth login`, `aws configure`, `az login`,
  `docker login`, etc., ask the user to run them via the `! <command>` prefix in their prompt so output
  lands in this session — don't try to drive an interactive credential prompt yourself.
- **Confirm before anything that costs money or is outward-facing** (creating cloud resources, pushing
  images, `terraform apply`). Summarize what will be created and the rough cost first.
- **Don't over-promise.** If the adapter for a chosen backend/target isn't built yet (see the status
  column below), say so and offer to implement it first (that's Phase 5a–5f in `docs/PACKAGING.md`) or
  fall back to a supported option. Verify availability by checking the workspace, not by assuming.
- Prefer the smallest thing that works; default to **local SQLite + Docker Compose** when the user is
  unsure, then offer to graduate to cloud.

## Step 0 — Preflight
Detect the environment before asking anything:
- OS/shell; `git` repo present; `cargo`/`rustc`; `docker`/`docker compose`; cloud CLIs (`gcloud`, `aws`,
  `az`); `kubectl`/`helm`; `claude` (for the judge engine).
- Report what's present/missing in a short table. Offer to install or work around gaps (e.g. use the
  prebuilt container image instead of building from source if Rust is missing).

## Step 1 — Pick the stack (use AskUserQuestion)
Ask these in order; skip ones already implied. Recommend a sensible default per question.
1. **Deploy target** — Local (Docker Compose) · Serverless container (Cloud Run / App Runner / Container
   Apps) · Kubernetes (Helm) · Bare VM/binary.
2. **Database** — see the catalog below; recommend SQLite for local, Postgres (Neon/Supabase free tier)
   for cloud. Note Firestore and BigQuery are GCP-specific.
3. **Cloud provider** (if not local) — GCP · AWS · Azure · Other/none.
4. **Auth mode** — `dev` (relaxed, localhost) or `enforced` (admin key + per-project API keys). Default
   `enforced` for anything cloud-facing.
5. **LLM providers to wire** — Anthropic via `claude -p` (judge, default) · OpenAI · Google Gemini
   (for multi-provider benchmark generation; needs each provider's API key).

Echo back the chosen stack as a one-line summary and confirm before proceeding.

## Step 2 — Collect credentials (only what the choices need)
From the catalog, list exactly which secrets/CLI logins their selections require and how to provide each:
- DB connection string → `LIGHTTRACK_DATABASE_URL` (or `LIGHTTRACK_DB` for the SQLite path).
- `LIGHTTRACK_ADMIN_KEY` (generate a strong random one for `enforced` mode).
- Provider API keys → `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` (only those selected).
- Cloud auth → ask the user to run the interactive login via `!`.
Write non-interactive values into a git-ignored `.env` (and confirm it's ignored). For cloud deploys,
prefer the platform's secret store and reference secrets by name.

## Step 3 — Integrate (configure → migrate → build)
- Set the config: `LIGHTTRACK_BIND`, `LIGHTTRACK_DATABASE_URL`/`LIGHTTRACK_DB`, `LIGHTTRACK_PRICING`,
  `LIGHTTRACK_AUTH_MODE`, `LIGHTTRACK_ADMIN_KEY`.
- Initialize/migrate the schema for the chosen backend (SQLite auto-creates; for SQL backends run the
  migrations; for BigQuery/Firestore apply their setup).
- Build: `cargo build --release` (or pull the container image). Confirm a clean build.

## Step 4 — Deploy (do it for them)
Follow the path for the chosen target (details in `docs/PACKAGING.md` §4):
- **Local:** `docker compose up -d` (api + runner + chosen DB + Grafana), or run the binaries directly.
- **Serverless container:** build+push the image, then `gcloud run deploy` / `aws apprunner` /
  `az containerapp up`, injecting secrets and the DB URL. Confirm cost first.
- **Kubernetes:** `helm install lighttrack deploy/helm/lighttrack -f values.yaml`.
- The **runner** (`lt-runner`, the `claude -p` engine) runs where `claude`/keys are available — the
  user's machine or a small VM — not on serverless. Wire it to the API base URL + key.

## Step 5 — Verify
- Hit `/health`; in `enforced` mode confirm a no-key request is `401`.
- Create a project + API key (`lt projects create`, `lt keys create`).
- Send a synthetic event and read it back (`POST /v1/events` → `GET /v1/events`, `GET /v1/costs`).
- Optionally run one `lt-runner score-text` to confirm the judge engine + provider key work.
- Print the final URLs, the admin key location, and the MCP `.mcp.json` setup for Claude Code.

## Step 6 — Next steps
Point the user to: adding more projects/keys, building a dataset from real traffic
(`lt-runner dataset build`), defining a rubric + benchmark, and the Grafana dashboard.

---

## Stack catalog — the selectable options
Check the **Status** before promising a path; "planned" means the adapter is part of Phase 5 and may need
to be implemented first (offer to do it).

### Databases
| Option | Selector | Runs where | Credentials needed | Status |
|---|---|---|---|---|
| SQLite | `LIGHTTRACK_DB=./data/lt.db` | local / single VM | none | **available** |
| DuckDB | `duckdb://…` | local analytical | none | planned |
| libSQL / Turso | `libsql://…` | local / edge | Turso token | planned |
| Postgres (self/RDS/Cloud SQL/Azure DB) | `postgres://…` | any cloud | DB URL (+ TLS) | planned (5a) |
| Neon / Supabase (serverless PG) | `postgres://…` | any (cloud-neutral) | connection string | planned (5a) |
| Firestore | `firestore://project` | GCP | GCP service account / ADC | planned (full-scope) |
| BigQuery (analytical sink) | `bigquery://project/dataset` | GCP | GCP service account / ADC | planned |

> Firestore is a **supported full-scope option**, not dropped — it's the GCP-native config store
> alongside Postgres. Postgres is the cross-cloud default; pick Firestore when the user is all-in on GCP.

### Deploy targets
| Target | Tooling | Clouds | Credentials | Status |
|---|---|---|---|---|
| Local Docker Compose | `docker compose up` | — | none | planned (5b) |
| Serverless container | image + Cloud Run / App Runner / Container Apps | GCP/AWS/Azure | cloud login | planned (5d) |
| Kubernetes | Helm chart | EKS/GKE/AKS/any | kubeconfig | planned (5e) |
| Bare binary | `cargo run` / prebuilt | — | none | **available** |

### LLM providers
| Provider | Used for | Key | Status |
|---|---|---|---|
| Anthropic (`claude -p`) | judge engine (default) | subscription OAuth or `ANTHROPIC_API_KEY` | **available** |
| OpenAI | multi-provider generation | `OPENAI_API_KEY` | planned (3.6e) |
| Google Gemini | multi-provider generation | `GEMINI_API_KEY` | planned (3.6e) |

## References
`docs/PACKAGING.md` (multicloud/multi-DB design + Phase 5a–5f) · `docs/ARCHITECTURE.md` ·
`docs/ROADMAP.md` · `config/lighttrack.example.toml`.
