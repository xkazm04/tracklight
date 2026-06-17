//! `compare` — the runner's multi-target benchmark leaderboard (quality × cost × latency). Shared so
//! `lt-runner bench` (compare mode), the CLI, and MCP all emit the same table instead of a bespoke one.
//!
//! Input shape (built by the runner): `{ "n_cases": N, "targets": [ {label, mean, pass_rate,
//! agreement, gen_cost_usd, judge_cost_usd, p50_latency_ms, errored} ] }`.

use serde_json::Value;

use crate::md::{f, money, opt_u, pct, s, u, Align, Table};

pub(crate) fn leaderboard(v: &Value) -> Option<String> {
    let targets = v.get("targets")?.as_array()?;
    if targets.is_empty() {
        return Some("_No comparison targets._".to_string());
    }
    let n_cases = v.get("n_cases").and_then(Value::as_u64).unwrap_or(0);

    let mut t = Table::new(&[
        ("Target", Align::Left),
        ("Mean", Align::Right),
        ("Pass%", Align::Right),
        ("Agree", Align::Right),
        ("Gen$", Align::Right),
        ("Judge$", Align::Right),
        ("p50", Align::Right),
        ("Err", Align::Right),
    ]);
    // Best = highest mean among targets that didn't error out every case (mirrors the runner's rule).
    let mut best: Option<(&str, f64)> = None;
    for r in targets {
        let label = s(r, "label");
        let mean = f(r, "mean");
        let errored = u(r, "errored");
        if errored < n_cases && best.map_or(true, |(_, bm)| mean > bm) {
            best = Some((label, mean));
        }
        t.row(vec![
            label.to_string(),
            format!("{mean:.2}"),
            pct(f(r, "pass_rate")),
            format!("{:.2}", f(r, "agreement")),
            money(f(r, "gen_cost_usd")),
            money(f(r, "judge_cost_usd")),
            opt_u(r, "p50_latency_ms").map(|m| format!("{m}ms")).unwrap_or_else(|| "—".into()),
            errored.to_string(),
        ]);
    }
    let mut out = format!("### Comparison — {n_cases} case(s)\n\n{}", t.render());
    if let Some((label, mean)) = best {
        out.push_str(&format!("\n**Best mean: {label} ({mean:.2})**\n"));
    }
    Some(out)
}
