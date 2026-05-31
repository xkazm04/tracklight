//! Small shared helpers: percentiles, dimension means, token-priced cost, claude resolution.

use std::collections::HashMap;

use serde_json::Value;

use lighttrack_core::ModelPriceRow;

/// p50/p95 of a latency sample (nearest-rank). Returns (None, None) if empty.
pub(crate) fn percentiles(latencies: &mut [u64]) -> (Option<u64>, Option<u64>) {
    if latencies.is_empty() {
        return (None, None);
    }
    latencies.sort_unstable();
    let pick = |p: f64| {
        let idx = (((latencies.len() - 1) as f64) * p).round() as usize;
        latencies[idx.min(latencies.len() - 1)]
    };
    (Some(pick(0.50)), Some(pick(0.95)))
}

/// Mean score of a dimension across `n` cases.
pub(crate) fn dim_mean(sums: &HashMap<String, f64>, key: &str, n: u32) -> f64 {
    sums.get(key).copied().unwrap_or(0.0) / n.max(1) as f64
}

/// Cost of a call from the DB price book (used when the provider API returns no $ cost).
pub(crate) fn price_gen_cost(
    prices: &[ModelPriceRow],
    provider: &str,
    model: &str,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
) -> f64 {
    prices
        .iter()
        .find(|p| p.provider == provider && p.model == model)
        .map(|p| {
            (input_tokens.unwrap_or(0) as f64) * p.input_per_mtok / 1_000_000.0
                + (output_tokens.unwrap_or(0) as f64) * p.output_per_mtok / 1_000_000.0
        })
        .unwrap_or(0.0)
}

/// Render a JSON value as plain text (strings as-is; everything else compact JSON).
pub(crate) fn value_to_text(v: &Value) -> String {
    match v.as_str() {
        Some(s) => s.to_string(),
        None => v.to_string(),
    }
}

/// First 8 chars of an id, for compact logging.
pub(crate) fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Resolve a runnable claude executable. A child process can't invoke the npm `.cmd`/`.ps1` shims
/// with our quote-heavy args, so on Windows we prefer the real `claude.exe` the shim wraps.
pub(crate) fn resolve_claude_bin(given: &str) -> String {
    if given != "claude" {
        return given.to_string();
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let p =
                format!("{appdata}\\npm\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe");
            if std::path::Path::new(&p).exists() {
                return p;
            }
        }
    }
    given.to_string()
}
