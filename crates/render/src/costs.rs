//! `get_cost_summary` — usage rollup grouped by project + provider + model, sorted by spend.

use serde_json::Value;

use crate::md::{commafy, f, money, s, u, Align, Table};

pub(crate) fn summary(v: &Value) -> Option<String> {
    let rows = v.as_array()?;
    if rows.is_empty() {
        return Some("_No usage recorded yet._".to_string());
    }
    let mut sorted: Vec<&Value> = rows.iter().collect();
    sorted.sort_by(|a, b| f(b, "cost_usd").total_cmp(&f(a, "cost_usd")));

    let mut t = Table::new(&[
        ("Project", Align::Left),
        ("Provider", Align::Left),
        ("Model", Align::Left),
        ("Calls", Align::Right),
        ("In tok", Align::Right),
        ("Out tok", Align::Right),
        ("Cost", Align::Right),
    ]);
    let (mut calls, mut in_t, mut out_t, mut cost) = (0u64, 0u64, 0u64, 0.0f64);
    for r in &sorted {
        let c = u(r, "calls");
        let i = u(r, "input_tokens");
        let o = u(r, "output_tokens");
        let cu = f(r, "cost_usd");
        calls += c;
        in_t += i;
        out_t += o;
        cost += cu;
        t.row(vec![
            s(r, "project_id").to_string(),
            s(r, "provider").to_string(),
            s(r, "model").to_string(),
            commafy(c),
            commafy(i),
            commafy(o),
            money(cu),
        ]);
    }
    Some(format!(
        "{}\n**Total: {} across {} calls** ({} in / {} out tokens)\n",
        t.render(),
        money(cost),
        commafy(calls),
        commafy(in_t),
        commafy(out_t),
    ))
}
