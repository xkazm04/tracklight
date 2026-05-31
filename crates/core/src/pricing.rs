use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{LtError, Result};
use crate::event::{Provider, TokenUsage};

/// A persisted price-book row (the DB-backed source of truth; `pricing.json` is just the seed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPriceRow {
    pub provider: String,
    pub model: String,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_mtok: Option<f64>,
    #[serde(default = "Utc::now")]
    pub effective_date: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// Per-model price, in USD per 1,000,000 tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    /// Discounted rate for cached/prompt-cache input tokens. Falls back to `input_per_mtok` if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_mtok: Option<f64>,
}

/// A book of model prices keyed by `"<provider>/<model>"`.
#[derive(Debug, Clone, Default)]
pub struct PriceBook {
    entries: HashMap<String, ModelPrice>,
}

/// Shape of `config/pricing.json`.
#[derive(Debug, Deserialize)]
struct PricingFile {
    models: HashMap<String, ModelPrice>,
}

impl PriceBook {
    pub fn new(entries: HashMap<String, ModelPrice>) -> Self {
        Self { entries }
    }

    /// Parse the on-disk `pricing.json` (the `{ "models": { ... } }` form).
    pub fn from_json_str(s: &str) -> Result<Self> {
        let parsed: PricingFile =
            serde_json::from_str(s).map_err(|e| LtError::InvalidPriceBook(e.to_string()))?;
        Ok(Self::new(parsed.models))
    }

    pub fn key(provider: Provider, model: &str) -> String {
        format!("{}/{}", provider.as_str(), model)
    }

    /// Build a price book from persisted rows (keyed `"<provider>/<model>"`).
    pub fn from_rows(rows: &[ModelPriceRow]) -> Self {
        let entries = rows
            .iter()
            .map(|r| {
                (
                    format!("{}/{}", r.provider, r.model),
                    ModelPrice {
                        input_per_mtok: r.input_per_mtok,
                        output_per_mtok: r.output_per_mtok,
                        cached_input_per_mtok: r.cached_input_per_mtok,
                    },
                )
            })
            .collect();
        Self { entries }
    }

    /// Flatten this book into rows (for seeding the DB from `pricing.json`).
    pub fn rows(&self) -> Vec<ModelPriceRow> {
        self.entries
            .iter()
            .filter_map(|(k, v)| {
                let (provider, model) = k.split_once('/')?;
                Some(ModelPriceRow {
                    provider: provider.to_string(),
                    model: model.to_string(),
                    input_per_mtok: v.input_per_mtok,
                    output_per_mtok: v.output_per_mtok,
                    cached_input_per_mtok: v.cached_input_per_mtok,
                    effective_date: Utc::now(),
                    source_url: None,
                })
            })
            .collect()
    }

    /// Look up a price, trying an exact `provider/model` match first, then a date-suffix-trimmed
    /// fallback (e.g. `claude-haiku-4-5-20251001` → `claude-haiku-4-5`).
    pub fn lookup(&self, provider: Provider, model: &str) -> Option<&ModelPrice> {
        if let Some(p) = self.entries.get(&Self::key(provider, model)) {
            return Some(p);
        }
        let trimmed = trim_date_suffix(model);
        if trimmed != model {
            return self.entries.get(&Self::key(provider, trimmed));
        }
        None
    }

    /// Compute cost in USD for the given usage, or `None` if the model is unpriced.
    /// Cached input tokens are billed at the cached rate when one exists; otherwise at the input rate.
    pub fn cost_usd(&self, provider: Provider, model: &str, usage: &TokenUsage) -> Option<f64> {
        let p = self.lookup(provider, model)?;
        let cached = usage.cached_input.unwrap_or(0);
        let billable_input = usage.input.saturating_sub(cached);

        let mut cost = (billable_input as f64) * p.input_per_mtok / 1_000_000.0
            + (usage.output as f64) * p.output_per_mtok / 1_000_000.0;

        let cached_rate = p.cached_input_per_mtok.unwrap_or(p.input_per_mtok);
        cost += (cached as f64) * cached_rate / 1_000_000.0;

        Some(cost)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Strip a trailing `-YYYYMMDD` date suffix if present.
fn trim_date_suffix(model: &str) -> &str {
    if let Some((head, tail)) = model.rsplit_once('-') {
        if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) {
            return head;
        }
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> PriceBook {
        let mut m = HashMap::new();
        m.insert(
            "anthropic/claude-haiku-4-5".to_string(),
            ModelPrice {
                input_per_mtok: 1.0,
                output_per_mtok: 5.0,
                cached_input_per_mtok: Some(0.1),
            },
        );
        PriceBook::new(m)
    }

    #[test]
    fn computes_cost_with_cache() {
        let b = book();
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cached_input: Some(500_000),
            reasoning: None,
        };
        // billable input 500k @1.0 = 0.5, cached 500k @0.1 = 0.05, output 1M @5.0 = 5.0
        let c = b.cost_usd(Provider::Anthropic, "claude-haiku-4-5", &usage).unwrap();
        assert!((c - 5.55).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn date_suffix_fallback() {
        let b = book();
        assert!(b
            .lookup(Provider::Anthropic, "claude-haiku-4-5-20251001")
            .is_some());
    }

    #[test]
    fn unknown_model_is_none() {
        assert!(book().cost_usd(Provider::OpenAi, "nope", &TokenUsage::default()).is_none());
    }
}
