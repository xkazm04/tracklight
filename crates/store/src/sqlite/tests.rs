use chrono::Utc;
use serde_json::Value;

use lighttrack_core::{
    new_id, ApiKey, Job, LimitAction, LimitMetric, LimitRule, LimitWindow, LlmEvent, Operation,
    Project, Provider, Redaction, Status, TokenUsage,
};

use super::SqliteStore;
use crate::Store;

fn ev(project: &str, model: &str, inp: u64, out: u64, cost: f64) -> LlmEvent {
    LlmEvent {
        id: new_id(),
        project_id: project.into(),
        trace_id: Some("trace-1".into()),
        span_id: None,
        parent_span_id: None,
        ts: Utc::now(),
        provider: Provider::Anthropic,
        model: model.into(),
        operation: Operation::Chat,
        usage: TokenUsage {
            input: inp,
            output: out,
            cached_input: None,
            reasoning: None,
        },
        cost_usd: Some(cost),
        latency_ms: Some(123),
        status: Status::Success,
        error: None,
        input: None,
        output: None,
        tags: vec!["smoke".into()],
        source: Some("test".into()),
        metadata: serde_json::json!({"k":"v"}),
    }
}

#[test]
fn insert_list_cost_roundtrip() {
    let s = SqliteStore::open_in_memory().unwrap();
    s.insert_event(&ev("p1", "claude-haiku-4-5", 100, 50, 0.001)).unwrap();
    s.insert_event(&ev("p1", "claude-haiku-4-5", 200, 80, 0.002)).unwrap();
    s.insert_event(&ev("p2", "claude-opus-4-8", 10, 5, 0.01)).unwrap();

    assert_eq!(s.list_events(None, 10).unwrap().len(), 3);
    let p1 = s.list_events(Some("p1"), 10).unwrap();
    assert_eq!(p1.len(), 2);
    assert_eq!(p1[0].project_id, "p1");
    assert_eq!(p1[0].tags, vec!["smoke".to_string()]);
    assert_eq!(p1[0].metadata, serde_json::json!({"k":"v"}));

    let costs = s.cost_summary(Some("p1")).unwrap();
    assert_eq!(costs.len(), 1);
    assert_eq!(costs[0].calls, 2);
    assert_eq!(costs[0].input_tokens, 300);
    assert!((costs[0].cost_usd - 0.003).abs() < 1e-9);
}

#[test]
fn projects_keys_limits_usage() {
    let s = SqliteStore::open_in_memory().unwrap();
    let now = Utc::now();

    let proj = Project {
        id: "p1".into(),
        name: "demo".into(),
        enabled: true,
        redaction: Redaction::None,
        created_at: now,
    };
    s.create_project(&proj).unwrap();
    assert_eq!(s.list_projects().unwrap().len(), 1);
    assert!(s.get_project("p1").unwrap().is_some());
    assert!(s.get_project("nope").unwrap().is_none());

    let key = ApiKey {
        id: "k1".into(),
        project_id: "p1".into(),
        name: "default".into(),
        prefix: "abc12345".into(),
        key_hash: "salt:hash".into(),
        created_at: now,
        last_used_at: None,
        revoked: false,
    };
    s.create_api_key(&key).unwrap();
    assert_eq!(s.find_api_key_by_prefix("abc12345").unwrap().unwrap().project_id, "p1");
    assert!(s.find_api_key_by_prefix("zzz").unwrap().is_none());

    let rule = LimitRule {
        id: "r1".into(),
        project_id: "p1".into(),
        metric: LimitMetric::CostUsd,
        window: LimitWindow::Hour,
        threshold: 0.005,
        action: LimitAction::Alert,
        enabled: true,
    };
    s.create_limit_rule(&rule).unwrap();
    assert_eq!(s.list_limit_rules("p1", true).unwrap().len(), 1);

    s.insert_event(&ev("p1", "claude-haiku-4-5", 1000, 500, 0.0035)).unwrap();
    s.insert_event(&ev("p1", "claude-haiku-4-5", 2000, 200, 0.00165)).unwrap();

    let u = s.usage_since("p1", LimitWindow::Hour.since(Utc::now())).unwrap();
    assert_eq!(u.calls, 2);
    assert_eq!(u.tokens, 3700);
    assert!((u.cost_usd - 0.00515).abs() < 1e-9);
    assert!(rule.evaluate(u.cost_usd).breached);
}

#[test]
fn job_queue_claim_finish() {
    let s = SqliteStore::open_in_memory().unwrap();
    let now = Utc::now();
    let job = Job {
        id: "j1".into(),
        job_type: "bench_run".into(),
        payload: serde_json::json!({ "benchmark_id": "b1" }),
        status: "queued".into(),
        attempts: 0,
        max_attempts: 3,
        progress: None,
        error: None,
        result: Value::Null,
        claimed_at: None,
        created_at: now,
        updated_at: now,
    };
    s.create_job(&job).unwrap();

    let claimed = s.claim_job(now).unwrap().unwrap();
    assert_eq!(claimed.id, "j1");
    assert_eq!(claimed.status, "running");
    assert_eq!(claimed.attempts, 1);
    assert_eq!(claimed.payload["benchmark_id"], "b1");

    assert!(s.claim_job(now - chrono::Duration::seconds(1)).unwrap().is_none());

    s.finish_job("j1", "done", &serde_json::json!({ "run_id": "r1" }), None).unwrap();
    let got = s.get_job("j1").unwrap().unwrap();
    assert_eq!(got.status, "done");
    assert_eq!(got.result["run_id"], "r1");
    assert_eq!(s.list_jobs(Some("done"), 10).unwrap().len(), 1);
}
