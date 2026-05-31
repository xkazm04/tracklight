//! `score` / `score-text`: judge stored events or an ad-hoc input/output pair.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use lighttrack_core::LlmEvent;
use lighttrack_engine::{build_judge_prompt, parse_judge_spec, run_judge, EngineConfig, JudgeOutcome};

use crate::cli::Cli;
use crate::http::{get, post};
use crate::util::{short, value_to_text};

/// Score the most recent events (with both input and output) for a project.
pub(crate) fn score_recent(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    engine: &EngineConfig,
    rubric: &str,
    project: Option<&str>,
    limit: usize,
) -> Result<()> {
    let mut path = format!("/v1/events?limit={limit}");
    if let Some(p) = project {
        path.push_str(&format!("&project={p}"));
    }
    let events: Vec<LlmEvent> = get(cli, http, &path)?;
    println!("fetched {} event(s) for scoring", events.len());
    let (jp, jm) = parse_judge_spec(&engine.model);

    let mut scored = 0;
    for ev in &events {
        let (input, output) = match (ev.input.as_ref(), ev.output.as_ref()) {
            (Some(i), Some(o)) => (value_to_text(i), value_to_text(o)),
            _ => {
                println!("  - {} skipped (no input/output content)", short(&ev.id));
                continue;
            }
        };
        print!("  - judging {} ({})... ", short(&ev.id), ev.model);
        let outcome = judge_one(engine, &jp, &jm, rubric, &input, &output)?;
        let score = build_score(&ev.project_id, Some(&ev.id), rubric, &outcome);
        post(cli, http, "/v1/scores", &score)?;
        scored += 1;
        println!(
            "score={:.2}/{:.0} pass={} cost={} :: {}",
            outcome.verdict.score,
            outcome.verdict.max,
            outcome.verdict.pass,
            outcome
                .cost_usd
                .map(|c| format!("${c:.5}"))
                .unwrap_or_else(|| "n/a".into()),
            outcome.verdict.reasoning
        );
    }
    println!("done: {scored} scored, {} skipped", events.len() - scored);
    Ok(())
}

/// Score a single ad-hoc input/output pair.
pub(crate) fn score_text(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    engine: &EngineConfig,
    rubric: &str,
    input: &str,
    output: &str,
    project: &str,
) -> Result<()> {
    let (jp, jm) = parse_judge_spec(&engine.model);
    let outcome = judge_one(engine, &jp, &jm, rubric, input, output)?;
    let score = build_score(project, None, rubric, &outcome);
    let stored = post(cli, http, "/v1/scores", &score)?;
    println!("posted score: {}", serde_json::to_string_pretty(&stored)?);
    Ok(())
}

fn judge_one(
    engine: &EngineConfig,
    provider: &str,
    model: &str,
    rubric: &str,
    input: &str,
    output: &str,
) -> Result<JudgeOutcome> {
    let prompt = build_judge_prompt(rubric, input, output);
    run_judge(engine, provider, model, &prompt).context("judge failed")
}

fn build_score(
    project_id: &str,
    event_id: Option<&str>,
    rubric: &str,
    outcome: &JudgeOutcome,
) -> Value {
    json!({
        "project_id": project_id,
        "event_id": event_id,
        "rubric": rubric,
        "value": outcome.verdict.score,
        "max": outcome.verdict.max,
        "pass": outcome.verdict.pass,
        "reasoning": outcome.verdict.reasoning,
        "scored_by": outcome.model,
        "cost_usd": outcome.cost_usd,
    })
}
