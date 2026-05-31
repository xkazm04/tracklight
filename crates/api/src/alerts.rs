//! Breach-alert delivery: when an ingested event trips a limit, push the breach to a configured
//! webhook and/or ntfy endpoint.
//!
//! Delivery is **best-effort** and happens **off the request path** (a spawned task), so a slow or
//! down alert sink never delays or fails ingest. Alerts are **deduplicated** per
//! `(project, metric, window)` with a cooldown, so a sustained breach (which trips on every ingest
//! until the rolling window clears) doesn't spam the channel.
//!
//! Config is server-global via env (per-project routing would need schema/Store changes — the
//! breach payload carries `project_id` so a single receiver can route):
//!   LIGHTTRACK_ALERT_WEBHOOK       POST a JSON body (Slack/Discord/custom) on breach
//!   LIGHTTRACK_ALERT_NTFY          POST a text body to an ntfy topic URL on breach
//!   LIGHTTRACK_ALERT_COOLDOWN_SECS re-alert window per (project, metric, window) (default 3600)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lighttrack_core::LimitStatus;

struct AlertConfig {
    webhook: Option<String>,
    ntfy: Option<String>,
    cooldown: Duration,
}

pub(crate) struct Alerter {
    config: AlertConfig,
    http: reqwest::Client,
    last_sent: Mutex<HashMap<String, Instant>>,
}

impl Alerter {
    pub(crate) fn from_env() -> Self {
        let cooldown = std::env::var("LIGHTTRACK_ALERT_COOLDOWN_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);
        Self {
            config: AlertConfig {
                webhook: env_opt("LIGHTTRACK_ALERT_WEBHOOK"),
                ntfy: env_opt("LIGHTTRACK_ALERT_NTFY"),
                cooldown: Duration::from_secs(cooldown),
            },
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            last_sent: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.config.webhook.is_some() || self.config.ntfy.is_some()
    }

    /// One-line summary for the startup banner.
    pub(crate) fn describe(&self) -> String {
        if !self.enabled() {
            return "off".to_string();
        }
        let mut chans = Vec::new();
        if self.config.webhook.is_some() {
            chans.push("webhook");
        }
        if self.config.ntfy.is_some() {
            chans.push("ntfy");
        }
        format!("{} (cooldown {}s)", chans.join("+"), self.config.cooldown.as_secs())
    }

    /// Fire best-effort delivery for the given breaches (after per-key cooldown dedup). Returns
    /// immediately; the actual HTTP happens on a spawned task.
    pub(crate) fn notify(self: &Arc<Self>, breaches: &[LimitStatus]) {
        if !self.enabled() {
            return;
        }
        let due: Vec<LimitStatus> = breaches.iter().filter(|b| self.should_send(b)).cloned().collect();
        if due.is_empty() {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move { me.deliver(due).await });
    }

    /// True if this breach is outside its cooldown (and records the send time).
    fn should_send(&self, b: &LimitStatus) -> bool {
        let key = format!("{}:{:?}:{:?}", b.project_id, b.metric, b.window);
        let now = Instant::now();
        let mut map = self.last_sent.lock().unwrap();
        match map.get(&key) {
            Some(t) if now.duration_since(*t) < self.config.cooldown => false,
            _ => {
                map.insert(key, now);
                true
            }
        }
    }

    async fn deliver(&self, breaches: Vec<LimitStatus>) {
        for b in &breaches {
            let msg = message(b);
            if let Some(url) = &self.config.webhook {
                self.post_webhook(url, &msg, b).await;
            }
            if let Some(url) = &self.config.ntfy {
                self.post_ntfy(url, &msg).await;
            }
        }
    }

    async fn post_webhook(&self, url: &str, msg: &str, b: &LimitStatus) {
        // `text` (Slack) + `content` (Discord) + structured fields (custom receivers).
        let body = serde_json::json!({
            "event": "limit_breach", "text": msg, "content": msg, "breach": b,
        });
        match self.http.post(url).json(&body).send().await {
            Ok(r) if !r.status().is_success() => eprintln!("[alert] webhook -> HTTP {}", r.status()),
            Err(e) => eprintln!("[alert] webhook error: {e}"),
            _ => {}
        }
    }

    async fn post_ntfy(&self, url: &str, msg: &str) {
        let req = self
            .http
            .post(url)
            .header("Title", "LightTrack limit breach")
            .header("Tags", "warning")
            .header("Priority", "high")
            .body(msg.to_string());
        match req.send().await {
            Ok(r) if !r.status().is_success() => eprintln!("[alert] ntfy -> HTTP {}", r.status()),
            Err(e) => eprintln!("[alert] ntfy error: {e}"),
            _ => {}
        }
    }
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn message(b: &LimitStatus) -> String {
    format!(
        "LightTrack alert: project '{}' breached {:?}/{:?} limit — current {:.4} >= threshold {:.4} \
         ({:.0}% of limit), action={:?}",
        b.project_id, b.metric, b.window, b.current, b.threshold, b.ratio * 100.0, b.action
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lighttrack_core::{LimitAction, LimitMetric, LimitWindow};

    fn alerter(cooldown_secs: u64) -> Alerter {
        Alerter {
            config: AlertConfig { webhook: Some("x".into()), ntfy: None, cooldown: Duration::from_secs(cooldown_secs) },
            http: reqwest::Client::new(),
            last_sent: Mutex::new(HashMap::new()),
        }
    }

    fn breach(project: &str) -> LimitStatus {
        LimitStatus {
            rule_id: "r1".into(),
            project_id: project.into(),
            metric: LimitMetric::CostUsd,
            window: LimitWindow::Hour,
            action: LimitAction::Alert,
            current: 2.0,
            threshold: 1.0,
            breached: true,
            ratio: 2.0,
        }
    }

    #[test]
    fn dedup_within_cooldown() {
        let a = alerter(3600);
        let b = breach("p1");
        assert!(a.should_send(&b)); // first send
        assert!(!a.should_send(&b)); // suppressed within cooldown
        assert!(a.should_send(&breach("p2"))); // different key still sends
    }

    #[test]
    fn zero_cooldown_always_sends() {
        let a = alerter(0);
        let b = breach("p1");
        assert!(a.should_send(&b));
        assert!(a.should_send(&b));
    }
}
