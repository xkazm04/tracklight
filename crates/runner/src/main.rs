//! LightTrack runner — the scoring/benchmark engine.
//!
//! Pulls scoring/benchmark jobs (in-proc channel locally → Pub/Sub on e2-micro) and runs the judge:
//!
//! ```text
//! claude -p "<rubric prompt>" \
//!   --bare --output-format json \
//!   --json-schema '<JudgeVerdict schema>'
//! ```
//!
//! It parses `structured_output` into a `JudgeVerdict` and `total_cost_usd` into `Score.cost_usd`.
//! The judge is **unbudgeted** by design; only monitored traffic is subject to limits.

use lighttrack_core::judge_verdict_schema;

fn main() {
    println!("lighttrack-runner v{} (scaffold)", env!("CARGO_PKG_VERSION"));
    let schema = judge_verdict_schema();
    println!("would invoke: claude -p \"<rubric>\" --bare --output-format json --json-schema '{schema}'");
    println!("TODO(phase3): job queue, spawn claude -p, parse verdict + cost, write Score");
}
