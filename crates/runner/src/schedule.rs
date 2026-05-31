//! `schedule`: periodically sample live events into frozen datasets (online sampling).
//!
//! Runs as a daemon (loop on `--interval`) or a single cycle (`--once`, for OS cron / Cloud
//! Scheduler / a systemd timer). Each cycle names the dataset after the newest sampled event, so it
//! is **idempotent**: if that window was already captured, the cycle is skipped — which means idle
//! periods (no new traffic) cost nothing, even across separate `--once` processes. And
//! `build_from_events` never creates an empty dataset.

use std::time::Duration;

use anyhow::Result;

use lighttrack_core::{Dataset, LlmEvent};
use lighttrack_engine::EngineConfig;

use crate::cli::Cli;
use crate::dataset::build_from_events;
use crate::http::get;
use crate::util::short;

#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    engine: &EngineConfig,
    project: &str,
    interval: u64,
    once: bool,
    n: usize,
    name_prefix: &str,
    llm_scrub: bool,
) -> Result<()> {
    println!(
        "lt-runner schedule: sampling '{project}' every {interval}s (once={once}, n={n}, prefix={name_prefix})"
    );
    loop {
        match run_cycle(cli, http, engine, project, n, name_prefix, llm_scrub) {
            Ok(Some(name)) => println!("cycle: built dataset {name}"),
            Ok(None) => println!("cycle: no new events to sample; skipped"),
            // A failed cycle (e.g. API briefly down) must not kill the daemon.
            Err(e) => eprintln!("cycle error (continuing): {e}"),
        }
        if once {
            break;
        }
        std::thread::sleep(Duration::from_secs(interval));
    }
    Ok(())
}

/// One sampling cycle. Returns the new dataset name, or `None` if skipped (nothing new to sample, or
/// this window was already captured).
fn run_cycle(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    engine: &EngineConfig,
    project: &str,
    n: usize,
    name_prefix: &str,
    llm_scrub: bool,
) -> Result<Option<String>> {
    let events: Vec<LlmEvent> = get(cli, http, &format!("/v1/events?project={project}&limit={n}"))?;
    // Watermark = newest event that carries an input (events come back newest-first).
    let newest = match events.iter().find(|e| e.input.is_some()) {
        Some(e) => e.id.clone(),
        None => return Ok(None),
    };
    let name = format!("{name_prefix}-{}", short(&newest));

    // Idempotent: if a dataset for this watermark already exists, this window is captured — skip.
    let existing: Vec<Dataset> = get(cli, http, &format!("/v1/projects/{project}/datasets"))?;
    if existing.iter().any(|d| d.name == name) {
        return Ok(None);
    }

    let built = build_from_events(cli, http, engine, project, &name, &events, llm_scrub)?;
    Ok((built > 0).then_some(name))
}
