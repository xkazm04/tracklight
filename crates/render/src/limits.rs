//! `get_limit_status` (live per-rule usage vs threshold) and `list_limits` (configured rules).

use serde_json::Value;

use crate::md::{commafy, f, money, pct, s, Align, Table};

pub(crate) fn status(v: &Value) -> Option<String> {
    let statuses = v.get("statuses")?.as_array()?;
    let project = s(v, "project_id");
    let throttled = v.get("throttled").and_then(Value::as_bool).unwrap_or(false);
    if statuses.is_empty() {
        return Some(format!("_No limit rules configured for `{project}`._"));
    }
    let mut t = Table::new(&[
        ("Metric", Align::Left),
        ("Window", Align::Left),
        ("Used", Align::Right),
        ("Threshold", Align::Right),
        ("Used %", Align::Right),
        ("Status", Align::Left),
    ]);
    for st in statuses {
        let metric = s(st, "metric");
        let current = f(st, "current");
        let threshold = f(st, "threshold");
        let ratio = f(st, "ratio");
        let breached = st.get("breached").and_then(Value::as_bool).unwrap_or(false);
        let (used, thr) = if metric == "cost_usd" {
            (money(current), money(threshold))
        } else {
            (commafy(current as u64), commafy(threshold as u64))
        };
        let badge = if breached {
            "❌ over"
        } else if ratio >= 0.8 {
            "⚠️ near"
        } else {
            "✅ ok"
        };
        t.row(vec![
            metric.to_string(),
            s(st, "window").to_string(),
            used,
            thr,
            pct(ratio),
            badge.to_string(),
        ]);
    }
    let header = if throttled {
        format!("### Limits — `{project}` ⚠️ **throttled**\n\n")
    } else {
        format!("### Limits — `{project}` ✅ within limits\n\n")
    };
    Some(format!("{header}{}", t.render()))
}

pub(crate) fn list(v: &Value) -> Option<String> {
    let rows = v.as_array()?;
    if rows.is_empty() {
        return Some("_No limit rules._".to_string());
    }
    let mut t = Table::new(&[
        ("Metric", Align::Left),
        ("Window", Align::Left),
        ("Threshold", Align::Right),
        ("Action", Align::Left),
        ("Enabled", Align::Left),
    ]);
    for r in rows {
        let metric = s(r, "metric");
        let threshold = f(r, "threshold");
        let thr = if metric == "cost_usd" {
            money(threshold)
        } else {
            commafy(threshold as u64)
        };
        let enabled = r.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        t.row(vec![
            metric.to_string(),
            s(r, "window").to_string(),
            thr,
            s(r, "action").to_string(),
            if enabled { "✅".into() } else { "—".into() },
        ]);
    }
    Some(t.render())
}
