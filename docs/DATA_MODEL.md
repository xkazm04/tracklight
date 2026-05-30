# LightTrack — Data Model

All times are UTC. IDs are UUIDv4 strings unless noted. The same logical model backs SQLite (local) and
BigQuery (cloud); see `schema/`.

## `events` — one normalized LLM call
The heart of the system. Emitted by monitored apps, normalized + costed by `api`.

| Field | Type | Notes |
|---|---|---|
| `id` | string (uuid) | event id |
| `project_id` | string | FK → projects |
| `trace_id` | string? | groups multiple calls in one logical operation (OTel-aligned) |
| `span_id` | string? | this call's span |
| `parent_span_id` | string? | parent span, for nested agent calls |
| `ts` | timestamp | when the call happened |
| `provider` | string | `openai` \| `anthropic` \| `google` \| `unknown` |
| `model` | string | e.g. `gpt-4.1`, `claude-opus-4-8`, `gemini-2.5-pro` |
| `operation` | string | `chat` \| `completion` \| `embedding` \| `other` |
| `input_tokens` | int | |
| `output_tokens` | int | |
| `cached_input_tokens` | int? | billed at cached rate when priced |
| `reasoning_tokens` | int? | o-series / thinking |
| `cost_usd` | float? | provider-reported or computed from PriceBook |
| `latency_ms` | int? | |
| `status` | string | `success` \| `error` \| `timeout` |
| `error` | string? | message when status≠success |
| `input` | json? | messages/prompt — optional, redactable per project |
| `output` | json? | completion — optional, redactable |
| `tags` | json (array) | freeform labels |
| `source` | string? | host / app instance |
| `metadata` | json | arbitrary app-supplied fields |

## `projects`
| Field | Type | Notes |
|---|---|---|
| `id` | string | |
| `name` | string | |
| `enabled` | bool | |
| `redaction` | string | `none` \| `hash` \| `drop` — how to store prompts/outputs |
| `created_at` | timestamp | |

## `api_keys`
| Field | Type | Notes |
|---|---|---|
| `id` | string | |
| `project_id` | string | FK |
| `name` | string | label |
| `prefix` | string | non-secret display prefix, e.g. `lt_ab12cd` |
| `key_hash` | string | salted SHA-256 of the secret; raw key shown once at creation |
| `created_at` | timestamp | |
| `last_used_at` | timestamp? | |
| `revoked` | bool | |

## `limit_rules`
| Field | Type | Notes |
|---|---|---|
| `id` | string | |
| `project_id` | string | FK |
| `metric` | string | `cost_usd` \| `calls` \| `tokens` |
| `window` | string | `hour` \| `day` \| `month` |
| `threshold` | float | |
| `action` | string | `alert` \| `throttle` \| `block` (block = advisory until gateway mode) |
| `enabled` | bool | |

## `scores` — LLM-as-judge results
| Field | Type | Notes |
|---|---|---|
| `id` | string | |
| `project_id` | string | FK |
| `event_id` | string? | scored event (null for benchmark-only) |
| `rubric` | string | rubric/metric name |
| `value` | float | |
| `max` | float | scale upper bound |
| `pass` | bool? | |
| `reasoning` | string? | judge rationale |
| `scored_by` | string | judge model, e.g. `claude-haiku-4-5` |
| `cost_usd` | float? | judge call cost (watched, never throttled) |
| `created_at` | timestamp | |

## `benchmarks` / `benchmark_runs`
| `benchmarks` | Type | | `benchmark_runs` | Type |
|---|---|---|---|---|
| `id` | string | | `id` | string |
| `project_id` | string | | `benchmark_id` | string |
| `name` | string | | `started_at` | timestamp |
| `rubric` | string | | `finished_at` | timestamp? |
| `judge_model` | string | | `n_cases` | int |
| `target` | json | | `mean_score` | float |
| `dataset_ref` | string | | `pass_rate` | float |
| `baseline_score` | float? | | `cost_usd` | float |
| `created_at` | timestamp | | `status` | string |

## Judge structured output (`--json-schema`)
`claude -p` returns this in `structured_output` (see `core::score::judge_verdict_schema`):
```json
{ "score": 0.0, "max": 1.0, "pass": true, "reasoning": "..." }
```
`api`/`runner` also read `total_cost_usd` and per-model `usage` from the `claude -p --output-format json`
envelope to populate `scores.cost_usd`.
