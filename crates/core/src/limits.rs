use serde::{Deserialize, Serialize};

/// What a limit measures over its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitMetric {
    CostUsd,
    Calls,
    Tokens,
}

/// Rolling window a limit is evaluated over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitWindow {
    Hour,
    Day,
    Month,
}

/// What happens when a limit is breached. `Block` is advisory until gateway/proxy mode exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitAction {
    Alert,
    Throttle,
    Block,
}

/// A per-project limit. Tripped by **monitored traffic only** — the scoring engine is exempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitRule {
    pub id: String,
    pub project_id: String,
    pub metric: LimitMetric,
    pub window: LimitWindow,
    pub threshold: f64,
    pub action: LimitAction,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Result of evaluating a rule against a current rolling value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitStatus {
    pub rule_id: String,
    pub project_id: String,
    pub metric: LimitMetric,
    pub window: LimitWindow,
    pub action: LimitAction,
    pub current: f64,
    pub threshold: f64,
    pub breached: bool,
    /// Fraction of the threshold used (1.0 == at limit). Useful for "approaching limit" warnings.
    pub ratio: f64,
}

impl LimitRule {
    /// Pure evaluation: given the project's current value for this rule's metric+window,
    /// decide whether the limit is breached. The caller computes `current` from the store.
    pub fn evaluate(&self, current: f64) -> LimitStatus {
        let ratio = if self.threshold > 0.0 {
            current / self.threshold
        } else {
            f64::INFINITY
        };
        LimitStatus {
            rule_id: self.id.clone(),
            project_id: self.project_id.clone(),
            metric: self.metric,
            window: self.window,
            action: self.action,
            current,
            threshold: self.threshold,
            breached: current >= self.threshold,
            ratio,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> LimitRule {
        LimitRule {
            id: "r1".into(),
            project_id: "p1".into(),
            metric: LimitMetric::CostUsd,
            window: LimitWindow::Day,
            threshold: 10.0,
            action: LimitAction::Alert,
            enabled: true,
        }
    }

    #[test]
    fn breaches_at_threshold() {
        assert!(rule().evaluate(10.0).breached);
        assert!(rule().evaluate(12.5).breached);
        assert!(!rule().evaluate(9.99).breached);
    }

    #[test]
    fn ratio_tracks_usage() {
        assert!((rule().evaluate(5.0).ratio - 0.5).abs() < 1e-9);
    }
}
