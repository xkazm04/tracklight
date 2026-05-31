use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One scored dimension of a rubric (e.g. correctness, completeness, faithfulness, concision).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricDimension {
    /// Stable key used in the judge's JSON output (must be a valid identifier-ish string).
    pub key: String,
    /// What this dimension measures.
    pub description: String,
    /// Relative weight in the overall score.
    #[serde(default = "default_weight")]
    pub weight: f64,
    /// Anchored level descriptions, e.g. ["1.0 = fully correct & verifiable", "0.5 = minor error", "0 = wrong"].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<String>,
    /// Gating floor: if this dimension scores below it, the case fails regardless of the overall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<f64>,
}

fn default_weight() -> f64 {
    1.0
}

/// A weighted, anchored rubric — the judge's scoring contract (see docs/BENCHMARK_FRAMEWORK.md §3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rubric {
    #[serde(default = "crate::new_id")]
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    pub name: String,
    pub dimensions: Vec<RubricDimension>,
    /// Overall pass threshold (weighted score, 0–1).
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

fn default_threshold() -> f64 {
    0.7
}
