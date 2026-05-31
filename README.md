# LightTrack

A lightweight, self-hosted **LLM observability + scoring** tool. Think Langfuse, but headless-first,
data-open (raw SQL over everything), and using **Claude Code headless (`claude -p`) as a pluggable
scoring/benchmark engine**.

## What it does
- **Track** LLM calls from your apps across **OpenAI, Google (Gemini), and Anthropic** — running locally or in the cloud.
- **Cost** accounting per call / model / project, computed from a maintained price book.
- **Limits** per project (cost, calls, tokens over hour/day/month) that incoming traffic can trip → alerts (and an advisory throttle flag apps/MCP can read).
- **Score & benchmark** traces with an LLM-as-judge run through `claude -p` (structured `--json-schema` verdicts).
- **Notify** on limit breaches and score regressions.
- **MCP server** so Claude Code / agents can query LightTrack's data directly.
- **Headless API** + CLI for everything; optional **Looker Studio** dashboards on the cloud store.

## Status
`v0` scaffold. Core data types are implemented; services are skeletons we evolve over the coming days.
See [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Layout
```
crates/core     normalized event model, price book + cost calc, limits, scoring types  (implemented)
crates/api      ingest + query REST service (axum), runs on local box → Cloud Run        (skeleton)
crates/runner   drives the scoring/benchmark engine via `claude -p`                       (skeleton)
crates/mcp      MCP server exposing query/ops tools to Claude Code                         (skeleton)
crates/cli      operator CLI (query traces/costs, manage projects/keys, trigger runs)      (skeleton)
config/         pricing.json, lighttrack.example.toml
schema/         SQLite (local) + BigQuery (cloud) DDL
docs/           architecture, data model, roadmap, decisions
```

## Quick start (dev)
```powershell
cargo build
cargo run -p lighttrack-api      # placeholder banner for now
```

## Use from Claude Code (MCP)
`lt-mcp` is an MCP server exposing read tools (`list_projects`, `get_cost_summary`, `query_events`,
`get_limit_status`, `list_scores`) over the API. A project-scoped [`.mcp.json`](.mcp.json) is committed,
so after `cargo build` and starting the API on `:8787`, open Claude Code in this repo and approve the
`lighttrack` server — then ask things like *"what did project qa-demo spend?"* or *"show recent scores"*.

- Windows path is `target/debug/lt-mcp.exe`; on Linux/macOS change it to `target/debug/lt-mcp`.
- In `enforced` auth mode, add `"LIGHTTRACK_KEY": "<admin-or-project-key>"` to the server's `env`.
- Equivalent manual registration: `claude mcp add lighttrack -- <abs-path-to>/lt-mcp.exe`.

## Key facts to remember
- **Claude Code billing changes 2026-06-15:** headless `claude -p` no longer draws on the normal
  subscription — it meters against a separate monthly **Agent SDK credit** (Max 20x = $200/mo, no rollover)
  at API rates. LightTrack's judge runs against that credit. See [`docs/DECISIONS.md`](docs/DECISIONS.md).
- The **judge engine is unbudgeted** by design; **limits apply only to monitored (incoming) traffic**.
