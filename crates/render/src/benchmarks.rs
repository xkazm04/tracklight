//! `get_benchmark_runs` (run-history leaderboard + mean-score trend) and `list_benchmarks`.

use serde_json::Value;

use crate::md::{
    commafy, f, money, opt_f, opt_u, pct, s, short_ts, sparkline, status_glyph, trunc, u, Align,
    Table,
};

pub(crate) fn runs(v: &Value) -> Option<String> {
    let rows = v.as_array()?;
    if rows.is_empty() {
        return Some("_No runs yet._".to_string());
    }
    let mut t = Table::new(&[
        ("When", Align::Left),
        ("Status", Align::Left),
        ("Mean", Align::Right),
        ("Pass%", Align::Right),
        ("Cost", Align::Right),
        ("p50", Align::Right),
        ("Tokens", Align::Right),
        ("Cases", Align::Right),
    ]);
    let mut means: Vec<f64> = Vec::new();
    for r in rows {
        let status = s(r, "status");
        if let Some(m) = opt_f(r, "mean_score") {
            means.push(m);
        }
        let finished = s(r, "finished_at");
        let when = short_ts(if finished.is_empty() { s(r, "started_at") } else { finished });
        t.row(vec![
            when,
            format!("{} {status}", status_glyph(status)),
            opt_f(r, "mean_score").map(|m| format!("{m:.2}")).unwrap_or_else(|| "—".into()),
            opt_f(r, "pass_rate").map(pct).unwrap_or_else(|| "—".into()),
            money(f(r, "cost_usd")),
            opt_u(r, "p50_latency_ms").map(|m| format!("{m}ms")).unwrap_or_else(|| "—".into()),
            opt_u(r, "total_tokens").map(commafy).unwrap_or_else(|| "—".into()),
            u(r, "n_cases").to_string(),
        ]);
    }
    let mut header = format!("**{} run(s)**", rows.len());
    if means.len() >= 2 {
        let mut trend = means.clone();
        trend.reverse();
        header.push_str(&format!(" · mean trend `{}`", sparkline(&trend)));
    }
    Some(format!("{header}\n\n{}", t.render()))
}

pub(crate) fn detail(v: &Value) -> Option<String> {
    let id = s(v, "id");
    if !v.is_object() || id.is_empty() {
        return None;
    }
    let cases = v.get("dataset").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    let mut out = format!("### Benchmark `{}`\n\n", s(v, "name"));
    out.push_str(&format!("- **Id:** {id}\n"));
    out.push_str(&format!("- **Judge:** {}\n", s(v, "judge_model")));
    let rubric = if !s(v, "rubric_id").is_empty() {
        format!("structured (`{}`)", s(v, "rubric_id"))
    } else {
        trunc(s(v, "rubric"), 80)
    };
    out.push_str(&format!("- **Rubric:** {rubric}\n"));
    let cases_cell = if cases > 0 {
        cases.to_string()
    } else if !s(v, "dataset_ref").is_empty() {
        format!("dataset ref `{}`", s(v, "dataset_ref"))
    } else {
        "0".into()
    };
    out.push_str(&format!("- **Cases:** {cases_cell}\n"));
    if let Some(b) = opt_f(v, "baseline_score") {
        out.push_str(&format!("- **Baseline:** {b:.2}\n"));
    }
    out.push_str(&format!("- **Created:** {}\n", short_ts(s(v, "created_at"))));
    Some(out)
}

pub(crate) fn list(v: &Value) -> Option<String> {
    let rows = v.as_array()?;
    if rows.is_empty() {
        return Some("_No benchmarks._".to_string());
    }
    let mut t = Table::new(&[
        ("Name", Align::Left),
        ("Judge", Align::Left),
        ("Cases", Align::Right),
        ("Baseline", Align::Right),
        ("Benchmark id", Align::Left),
    ]);
    for r in rows {
        let cases = r.get("dataset").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
        let cases_cell = if cases > 0 {
            cases.to_string()
        } else if !s(r, "dataset_ref").is_empty() {
            "ref".into()
        } else {
            "0".into()
        };
        t.row(vec![
            trunc(s(r, "name"), 28),
            s(r, "judge_model").to_string(),
            cases_cell,
            opt_f(r, "baseline_score").map(|b| format!("{b:.2}")).unwrap_or_else(|| "—".into()),
            s(r, "id").to_string(),
        ]);
    }
    Some(t.render())
}
