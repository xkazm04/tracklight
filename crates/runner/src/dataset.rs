//! `dataset build`: sample real events, scrub PII (regex always + optional LLM pass), freeze.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use lighttrack_anon::scrub;
use lighttrack_core::LlmEvent;
use lighttrack_engine::{run_text, EngineConfig};

use crate::cli::Cli;
use crate::http::{get, post};
use crate::util::{short, value_to_text};

/// Sample events, scrub PII, and freeze a new dataset.
pub(crate) fn build_dataset(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    engine: &EngineConfig,
    project: &str,
    name: &str,
    n: usize,
    llm_scrub: bool,
) -> Result<()> {
    let events: Vec<LlmEvent> = get(cli, http, &format!("/v1/events?project={project}&limit={n}"))?;
    let with_input: Vec<&LlmEvent> = events.iter().filter(|e| e.input.is_some()).collect();
    println!(
        "sampling {} of {} event(s) with input from '{project}' (llm_scrub={llm_scrub})",
        with_input.len(),
        events.len()
    );

    let created: Value = post(
        cli,
        http,
        &format!("/v1/projects/{project}/datasets"),
        &json!({ "name": name, "source": "events:recent" }),
    )?;
    let dsid = created
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("dataset create returned no id"))?
        .to_string();

    let (mut built, mut total_redactions) = (0u32, 0usize);
    let method = if llm_scrub { "regex+llm" } else { "regex" };
    for ev in with_input {
        let (input_clean, r_in) =
            scrub_text(&value_to_text(ev.input.as_ref().unwrap()), llm_scrub, engine)?;
        let (output_clean, r_out) = match ev.output.as_ref() {
            Some(o) => {
                let (c, r) = scrub_text(&value_to_text(o), llm_scrub, engine)?;
                (Some(c), r)
            }
            None => (None, 0),
        };
        let redactions = r_in + r_out;
        total_redactions += redactions;
        let item = json!({
            "input": input_clean,
            "output": output_clean,
            "source_event_id": ev.id,
            "tags": ev.tags,
            "anonymization": { "method": method, "redactions": redactions },
        });
        post(cli, http, &format!("/v1/datasets/{dsid}/items"), &item)?;
        built += 1;
        println!("  + item from {} ({redactions} redactions)", short(&ev.id));
    }

    post(cli, http, &format!("/v1/datasets/{dsid}/freeze"), &json!({}))?;
    println!("\nbuilt dataset {dsid}: {built} items, {total_redactions} total redactions, frozen");
    Ok(())
}

/// Regex scrub (always) + optional LLM scrub pass. Returns (clean_text, redaction_count).
fn scrub_text(text: &str, llm: bool, engine: &EngineConfig) -> Result<(String, usize)> {
    let res = scrub(text);
    let mut out = res.text;
    let mut redactions = res.redactions;
    if llm {
        let prompt = format!(
            "Rewrite the text below, replacing any remaining personally identifiable information \
(names of people, organizations, precise locations, account/order numbers) with generic \
placeholders like <NAME>, <ORG>, <LOCATION>, <ID>. Preserve meaning and structure. \
Return ONLY the rewritten text, with no preamble.\n\nTEXT:\n{out}"
        );
        let outcome = run_text(engine, &prompt).context("LLM anonymization (claude -p) failed")?;
        let trimmed = outcome.text.trim();
        if !trimmed.is_empty() {
            let added = trimmed.matches('<').count().saturating_sub(out.matches('<').count());
            out = trimmed.to_string();
            redactions += added;
        }
    }
    Ok((out, redactions))
}
