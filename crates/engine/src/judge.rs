//! LLM-as-judge: provider-agnostic judging built on [`crate::generate`]. The judge is a structured
//! generation — we ask for JSON and parse the verdict from the model's text — so any provider judges.

use std::collections::HashMap;

use serde_json::Value;

use lighttrack_core::{JudgeVerdict, Rubric};

use crate::claude;
use crate::prompts::build_rubric_prompt;
use crate::providers::generate;
use crate::{DimScore, EngineConfig, EngineError, JudgeOutcome, Result, RubricOutcome, TextOutcome};

/// Parse a `[provider/]model` judge spec into (provider, model). No prefix => anthropic (claude -p).
pub fn parse_judge_spec(spec: &str) -> (String, String) {
    match spec.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => (p.to_string(), m.to_string()),
        _ => ("anthropic".to_string(), spec.to_string()),
    }
}

/// Extract the outermost `{...}` from a string (handles stray prose / code fences).
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    (end > start).then(|| s[start..=end].to_string())
}

/// Extract a JSON object from text into a Value (lenient; `Null` if none).
fn extract_json_value(s: &str) -> Value {
    extract_json_object(s)
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(Value::Null)
}

/// Weighted mean of (dimension, score) pairs using the rubric's weights.
fn weighted(dims: &[(String, f64)], rubric: &Rubric) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for (key, score) in dims {
        let w = rubric
            .dimensions
            .iter()
            .find(|d| &d.key == key)
            .map(|d| d.weight)
            .unwrap_or(1.0);
        num += score * w;
        den += w;
    }
    if den > 0.0 {
        num / den
    } else {
        0.0
    }
}

/// Run the judge on the given provider/model with a fully-formed prompt.
pub fn run_judge(
    cfg: &EngineConfig,
    provider: &str,
    model: &str,
    prompt: &str,
) -> Result<JudgeOutcome> {
    let g = generate(cfg, provider, model, None, prompt)?;
    let json = extract_json_object(&g.output)
        .ok_or_else(|| EngineError::Parse(format!("no JSON object in judge output: {}", g.output)))?;
    let verdict: JudgeVerdict = serde_json::from_str(&json)
        .map_err(|e| EngineError::Parse(format!("judge JSON not a verdict: {e}; got: {json}")))?;
    Ok(JudgeOutcome {
        verdict,
        cost_usd: g.cost_usd,
        model: g.model,
        session_id: None,
        latency_ms: g.latency_ms,
        input_tokens: g.input_tokens,
        output_tokens: g.output_tokens,
    })
}

/// Free-form text generation on Claude (anonymization / healing paragraphs).
pub fn run_text(cfg: &EngineConfig, prompt: &str) -> Result<TextOutcome> {
    let (envelope, latency_ms) = claude::invoke(cfg, prompt, &cfg.model, None, None)?;
    Ok(TextOutcome {
        text: envelope
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        cost_usd: envelope.get("total_cost_usd").and_then(Value::as_f64),
        model: claude::model_of(&envelope, &cfg.model),
        latency_ms,
    })
}

/// Judge one case against a rubric, averaging over `samples` (self-consistency). Overall + pass are
/// computed here (weighted dimensions + gating floors), never trusted to the model.
#[allow(clippy::too_many_arguments)]
pub fn run_rubric_judge(
    cfg: &EngineConfig,
    provider: &str,
    model: &str,
    rubric: &Rubric,
    input: &str,
    expected: Option<&str>,
    output: &str,
    samples: u32,
) -> Result<RubricOutcome> {
    let prompt = build_rubric_prompt(rubric, input, expected, output);
    let k = samples.max(1);

    let mut per_dim: HashMap<String, Vec<f64>> = HashMap::new();
    let mut reasonings: HashMap<String, String> = HashMap::new();
    let mut overalls: Vec<f64> = Vec::new();
    let (mut total_cost, mut any_cost, mut max_latency, mut in_tok, mut out_tok) =
        (0.0_f64, false, 0_u64, 0_u64, 0_u64);
    let mut model_used = model.to_string();

    for s in 0..k {
        let g = generate(cfg, provider, model, None, &prompt)?;
        let out = extract_json_value(&g.output);
        let mut sample: Vec<(String, f64)> = Vec::new();
        for d in &rubric.dimensions {
            let obj = out.get(&d.key);
            let score = obj
                .and_then(|o| o.get("score"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let reasoning = obj
                .and_then(|o| o.get("reasoning"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            per_dim.entry(d.key.clone()).or_default().push(score);
            if s == 0 {
                reasonings.insert(d.key.clone(), reasoning);
            }
            sample.push((d.key.clone(), score));
        }
        overalls.push(weighted(&sample, rubric));
        if let Some(c) = g.cost_usd {
            total_cost += c;
            any_cost = true;
        }
        if let Some(l) = g.latency_ms {
            max_latency = max_latency.max(l);
        }
        in_tok += g.input_tokens.unwrap_or(0);
        out_tok += g.output_tokens.unwrap_or(0);
        model_used = g.model;
    }

    let dimensions: Vec<DimScore> = rubric
        .dimensions
        .iter()
        .map(|d| {
            let v = per_dim.get(&d.key).cloned().unwrap_or_default();
            let mean = if v.is_empty() {
                0.0
            } else {
                v.iter().sum::<f64>() / v.len() as f64
            };
            DimScore {
                key: d.key.clone(),
                score: mean,
                reasoning: reasonings.get(&d.key).cloned().unwrap_or_default(),
                weight: d.weight,
            }
        })
        .collect();

    let overall = {
        let den: f64 = dimensions.iter().map(|d| d.weight).sum();
        if den > 0.0 {
            dimensions.iter().map(|d| d.score * d.weight).sum::<f64>() / den
        } else {
            0.0
        }
    };
    let pass = overall >= rubric.threshold
        && rubric.dimensions.iter().all(|d| {
            let s = dimensions
                .iter()
                .find(|x| x.key == d.key)
                .map(|x| x.score)
                .unwrap_or(0.0);
            d.floor.map_or(true, |f| s >= f)
        });
    let agreement = if k > 1 {
        let max = overalls.iter().cloned().fold(f64::MIN, f64::max);
        let min = overalls.iter().cloned().fold(f64::MAX, f64::min);
        (1.0 - (max - min)).clamp(0.0, 1.0)
    } else {
        1.0
    };

    Ok(RubricOutcome {
        dimensions,
        overall,
        pass,
        cost_usd: if any_cost { Some(total_cost) } else { None },
        latency_ms: Some(max_latency),
        tokens: Some(in_tok + out_tok),
        input_tokens: Some(in_tok),
        output_tokens: Some(out_tok),
        model: model_used,
        samples: k,
        agreement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_from_judge_text() {
        let text = "Here is my verdict:\n```json\n{\"score\":0.2,\"max\":1.0,\"pass\":false,\"reasoning\":\"wrong\"}\n```";
        let json = extract_json_object(text).unwrap();
        let v: JudgeVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(v.score, 0.2);
        assert!(!v.pass);
    }

    #[test]
    fn rubric_json_from_text() {
        let v = extract_json_value("noise {\"correctness\":{\"score\":0.9,\"reasoning\":\"ok\"}} tail");
        assert_eq!(v["correctness"]["score"], 0.9);
        assert!(extract_json_value("no json").is_null());
    }

    #[test]
    fn extracts_object() {
        assert_eq!(extract_json_object("noise {\"a\":1} tail"), Some("{\"a\":1}".to_string()));
        assert_eq!(extract_json_object("no json here"), None);
    }

    #[test]
    fn judge_spec_parsing() {
        assert_eq!(parse_judge_spec("haiku"), ("anthropic".into(), "haiku".into()));
        assert_eq!(
            parse_judge_spec("google/gemini-2.5-flash"),
            ("google".into(), "gemini-2.5-flash".into())
        );
    }
}
