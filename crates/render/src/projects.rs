//! `list_projects` — monitored applications/tenants. Full project ids are shown because the other
//! tools key off them, so the agent/operator can copy one straight into a follow-up call.

use serde_json::Value;

use crate::md::{s, short_ts, trunc, Align, Table};

pub(crate) fn list(v: &Value) -> Option<String> {
    let rows = v.as_array()?;
    if rows.is_empty() {
        return Some("_No projects._".to_string());
    }
    let mut t = Table::new(&[
        ("Name", Align::Left),
        ("On", Align::Left),
        ("Redaction", Align::Left),
        ("Created", Align::Left),
        ("Project id", Align::Left),
    ]);
    for r in rows {
        let enabled = r.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        t.row(vec![
            trunc(s(r, "name"), 28),
            if enabled { "✅".into() } else { "—".into() },
            s(r, "redaction").to_string(),
            short_ts(s(r, "created_at")),
            s(r, "id").to_string(),
        ]);
    }
    Some(t.render())
}
