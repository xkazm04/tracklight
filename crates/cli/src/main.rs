//! `lt` — LightTrack operator CLI (client of the API).
//!
//! Planned commands (Phase 2+):
//!   lt projects create|list
//!   lt keys create <project> --name ...        (prints the secret once)
//!   lt limits set <project> --metric cost_usd --window day --threshold 5 --action alert
//!   lt costs --project X --window day
//!   lt events --project X --since 1h
//!   lt bench run <benchmark_id>
//!
//! Triggerable from the OS (cron/Task Scheduler) or the user's Rust app.

fn main() {
    println!("lt (lighttrack-cli) v{} (scaffold)", env!("CARGO_PKG_VERSION"));
    println!("usage (planned): lt <projects|keys|limits|costs|events|bench> ...");
    println!("new id sample: {}", lighttrack_core::new_id());
    println!("TODO(phase2): clap commands talking to the API over HTTP");
}
