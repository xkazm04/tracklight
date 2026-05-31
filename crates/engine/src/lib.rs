//! The scoring engine: run `claude -p` as an LLM-as-judge and parse a [`JudgeVerdict`].
//!
//! Invocation:
//! ```text
//! claude -p "<prompt>" --output-format json --model <model> --json-schema '<JudgeVerdict schema>'
//! ```
//! We read `total_cost_usd` from the JSON envelope and the verdict from `structured_output`
//! (falling back to extracting a JSON object from the `result` text if a build doesn't return
//! `structured_output`). The judge is **unbudgeted** by design — callers never rate-limit it.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::{json, Map, Value};
use thiserror::Error;

use lighttrack_core::{JudgeVerdict, Rubric};

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("failed to spawn '{bin}': {source}")]
    Spawn {
        bin: String,
        source: std::io::Error,
    },
    #[error("claude exited with status {code}: {stderr}")]
    NonZero { code: i32, stderr: String },
    #[error("could not parse judge output: {0}")]
    Parse(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// How to invoke the judge.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub claude_bin: String,
    pub model: String,
    /// Pass `--bare` to skip auto-loading hooks/skills/MCP/CLAUDE.md. This avoids paying to
    /// re-cache ~40k tokens of context on every call (a one-word reply otherwise cost ~$0.05),
    /// but `--bare` bypasses subscription OAuth, so it requires `ANTHROPIC_API_KEY` in the env.
    pub bare: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            claude_bin: "claude".to_string(),
            model: "haiku".to_string(),
            bare: false,
        }
    }
}

/// The result of one judge call.
#[derive(Debug, Clone)]
pub struct JudgeOutcome {
    pub verdict: JudgeVerdict,
    pub cost_usd: Option<f64>,
    pub model: String,
    pub session_id: Option<String>,
    /// Wall-clock latency of the `claude -p` call.
    pub latency_ms: Option<u64>,
    /// Total input tokens (prompt + cache read + cache creation), if reported.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// The result of a free-form text call (e.g. LLM-based anonymization).
#[derive(Debug, Clone)]
pub struct TextOutcome {
    pub text: String,
    pub cost_usd: Option<f64>,
    pub model: String,
    pub latency_ms: Option<u64>,
}

/// One dimension's aggregated score within a rubric judgement.
#[derive(Debug, Clone)]
pub struct DimScore {
    pub key: String,
    pub score: f64,
    pub reasoning: String,
    pub weight: f64,
}

/// The result of judging one case against a [`Rubric`] (possibly averaged over k samples).
#[derive(Debug, Clone)]
pub struct RubricOutcome {
    pub dimensions: Vec<DimScore>,
    pub overall: f64,
    pub pass: bool,
    pub cost_usd: Option<f64>,
    pub latency_ms: Option<u64>,
    pub tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub model: String,
    pub samples: u32,
    /// Cross-sample agreement on the overall score (1.0 = identical; lower = judge disagreed).
    pub agreement: f64,
}

/// Build a judging prompt for an input/output pair against a rubric.
pub fn build_judge_prompt(rubric: &str, input: &str, output: &str) -> String {
    format!(
        "You are a strict evaluation judge. Evaluate the ASSISTANT OUTPUT for the given USER INPUT \
against the rubric below.\n\
Rubric: {rubric}\n\n\
Respond with ONLY a JSON object (no prose, no code fences) of the form:\n\
{{\"score\": <number 0.0-1.0>, \"max\": 1.0, \"pass\": <true|false>, \"reasoning\": \"<one sentence>\"}}\n\n\
=== USER INPUT ===\n{input}\n\n=== ASSISTANT OUTPUT ===\n{output}\n"
    )
}

/// Build a benchmark eval prompt for an input/output pair, with an optional reference answer.
pub fn build_eval_prompt(rubric: &str, input: &str, expected: Option<&str>, output: &str) -> String {
    let reference = match expected {
        Some(e) => format!("\n=== REFERENCE / EXPECTED ANSWER ===\n{e}\n"),
        None => String::new(),
    };
    format!(
        "You are a strict evaluation judge. Evaluate the ASSISTANT OUTPUT for the given USER INPUT \
against the rubric{ref_note}.\n\
Rubric: {rubric}\n\n\
Respond with ONLY a JSON object (no prose, no code fences):\n\
{{\"score\": <number 0.0-1.0>, \"max\": 1.0, \"pass\": <true|false>, \"reasoning\": \"<one sentence>\"}}\n\n\
=== USER INPUT ===\n{input}\n{reference}\n=== ASSISTANT OUTPUT ===\n{output}\n",
        ref_note = if expected.is_some() {
            " and the reference answer"
        } else {
            ""
        }
    )
}

/// Run `claude -p` with the given prompt/model, returning the parsed JSON envelope and wall-clock latency.
fn invoke(
    cfg: &EngineConfig,
    prompt: &str,
    model: &str,
    system_prompt: Option<&str>,
    schema: Option<&str>,
) -> Result<(Value, Option<u64>)> {
    let mut cmd = Command::new(&cfg.claude_bin);
    cmd.arg("-p")
        .arg(prompt)
        .arg("--output-format")
        .arg("json")
        .arg("--model")
        .arg(model)
        .stdin(Stdio::null()); // don't block waiting for piped stdin
    if let Some(sys) = system_prompt {
        cmd.arg("--append-system-prompt").arg(sys);
    }
    if let Some(s) = schema {
        cmd.arg("--json-schema").arg(s);
    }
    if cfg.bare {
        cmd.arg("--bare");
    }
    let started = Instant::now();
    let output = cmd.output().map_err(|source| EngineError::Spawn {
        bin: cfg.claude_bin.clone(),
        source,
    })?;
    let latency_ms = Some(started.elapsed().as_millis() as u64);

    if !output.status.success() {
        return Err(EngineError::NonZero {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let envelope: Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        EngineError::Parse(format!(
            "envelope not JSON: {e}; stdout was: {}",
            String::from_utf8_lossy(&output.stdout)
        ))
    })?;
    Ok((envelope, latency_ms))
}

fn token_counts(envelope: &Value) -> (Option<u64>, Option<u64>) {
    let usage = envelope.get("usage");
    let input = usage.map(|u| {
        let f = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        f("input_tokens") + f("cache_read_input_tokens") + f("cache_creation_input_tokens")
    });
    let output = usage.and_then(|u| u.get("output_tokens").and_then(Value::as_u64));
    (input, output)
}

fn model_of(envelope: &Value, cfg: &EngineConfig) -> String {
    envelope
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| cfg.model.clone())
}

/// Parse a `[provider/]model` judge spec into (provider, model). No prefix => anthropic (claude -p).
pub fn parse_judge_spec(spec: &str) -> (String, String) {
    match spec.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => (p.to_string(), m.to_string()),
        _ => ("anthropic".to_string(), spec.to_string()),
    }
}

/// Extract a JSON object from text into a Value (lenient; tolerates prose / code fences).
fn extract_json_value(s: &str) -> Value {
    extract_json_object(s)
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(Value::Null)
}

/// Run the judge on the given provider/model with a fully-formed prompt. The judge is just a
/// structured generation: we ask for JSON and parse the verdict from the model's text output.
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

/// Run a free-form text generation (no schema), returning the `result` text. Used by the optional
/// LLM anonymization pass.
pub fn run_text(cfg: &EngineConfig, prompt: &str) -> Result<TextOutcome> {
    let (envelope, latency_ms) = invoke(cfg, prompt, &cfg.model, None, None)?;
    Ok(TextOutcome {
        text: envelope
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        cost_usd: envelope.get("total_cost_usd").and_then(Value::as_f64),
        model: model_of(&envelope, cfg),
        latency_ms,
    })
}

// ---------------------------------------------------------------------------
// Rubric judging (Phase 3.6c)
// ---------------------------------------------------------------------------

/// Build a JSON schema keyed by dimension: each dimension yields `{score, reasoning}`.
pub fn build_rubric_schema(rubric: &Rubric) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for d in &rubric.dimensions {
        props.insert(
            d.key.clone(),
            json!({
                "type": "object",
                "properties": {
                    "score": { "type": "number", "description": format!("0.0-1.0 — {}", d.description) },
                    "reasoning": { "type": "string" }
                },
                "required": ["score", "reasoning"],
                "additionalProperties": false
            }),
        );
        required.push(Value::String(d.key.clone()));
    }
    let mut root = Map::new();
    root.insert("type".into(), json!("object"));
    root.insert("properties".into(), Value::Object(props));
    root.insert("required".into(), Value::Array(required));
    root.insert("additionalProperties".into(), json!(false));
    Value::Object(root)
}

/// RCAF judge prompt for a rubric: Role, Context (dimensions+anchors+reference), Action, Format.
pub fn build_rubric_prompt(
    rubric: &Rubric,
    input: &str,
    expected: Option<&str>,
    output: &str,
) -> String {
    let dims = rubric
        .dimensions
        .iter()
        .map(|d| {
            let anchors = if d.anchors.is_empty() {
                String::new()
            } else {
                format!(" Anchors: {}", d.anchors.join("; "))
            };
            format!("- {} (weight {}): {}.{}", d.key, d.weight, d.description, anchors)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let reference = expected
        .map(|e| format!("\n=== REFERENCE / EXPECTED ===\n{e}\n"))
        .unwrap_or_default();
    format!(
        "You are an impartial, strict evaluation judge. Score the ASSISTANT OUTPUT on EACH dimension \
below from 0.0 to 1.0 using the anchors. Penalize unnecessary length; do not reward verbosity. Judge \
only the output's quality for the input{ref_note}; ignore which model produced it.\n\n\
Dimensions:\n{dims}\n\n\
For each dimension return {{\"score\": <0.0-1.0>, \"reasoning\": \"<one sentence>\"}}.\n\n\
=== USER INPUT ===\n{input}\n{reference}\n=== ASSISTANT OUTPUT ===\n{output}\n",
        ref_note = if expected.is_some() { " and the reference" } else { "" }
    )
}

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

/// Judge one case against a rubric, averaging over `samples` (self-consistency). The overall score
/// and pass/fail are computed by us (weighted dimensions + gating floors), not trusted to the model.
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

/// Extract the outermost `{...}` from a string (handles stray prose / code fences).
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(s[start..=end].to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Generation (Phase 3.6e) — produce candidate outputs from a target model
// ---------------------------------------------------------------------------

/// The result of generating one candidate output from a target.
#[derive(Debug, Clone)]
pub struct GenOutcome {
    pub output: String,
    pub cost_usd: Option<f64>,
    pub model: String,
    pub latency_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// Generate a candidate output from a target. `anthropic` runs via `claude -p`; other providers
/// need an HTTPS adapter + API key (return a clear error until enabled — "Claude now, keys later").
pub fn generate(
    cfg: &EngineConfig,
    provider: &str,
    model: &str,
    system_prompt: Option<&str>,
    input: &str,
) -> Result<GenOutcome> {
    match provider {
        "anthropic" => {
            let (envelope, latency_ms) = invoke(cfg, input, model, system_prompt, None)?;
            let (input_tokens, output_tokens) = token_counts(&envelope);
            Ok(GenOutcome {
                output: envelope
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                cost_usd: envelope.get("total_cost_usd").and_then(Value::as_f64),
                model: envelope
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| model.to_string()),
                latency_ms,
                input_tokens,
                output_tokens,
            })
        }
        "google" => generate_gemini(model, system_prompt, input),
        "openai" => generate_openai(model, system_prompt, input),
        other => Err(EngineError::Other(format!("unknown provider '{other}'"))),
    }
}

/// Call the OpenAI Chat Completions API. Cost is left to the caller (price by tokens). Key from
/// OPENAI_API_KEY.
fn generate_openai(model: &str, system_prompt: Option<&str>, input: &str) -> Result<GenOutcome> {
    let key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| EngineError::Other("no OpenAI API key (set OPENAI_API_KEY)".into()))?;
    let mut messages = Vec::new();
    if let Some(sys) = system_prompt {
        messages.push(serde_json::json!({ "role": "system", "content": sys }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": input }));
    let body = serde_json::json!({ "model": model, "messages": messages });

    let client = reqwest::blocking::Client::new();
    let started = Instant::now();
    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(&key)
        .json(&body)
        .send()
        .map_err(|e| EngineError::Other(format!("openai request failed: {e}")))?;
    let latency_ms = Some(started.elapsed().as_millis() as u64);
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| EngineError::Other(format!("openai read failed: {e}")))?;
    if !status.is_success() {
        return Err(EngineError::Other(format!(
            "openai HTTP {}: {text}",
            status.as_u16()
        )));
    }
    let v: Value = serde_json::from_str(&text)?;
    let output = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let usage = v.get("usage");
    Ok(GenOutcome {
        output,
        cost_usd: None, // priced by the caller from the price book
        model: v
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| model.to_string()),
        latency_ms,
        input_tokens: usage.and_then(|u| u.get("prompt_tokens")).and_then(Value::as_u64),
        output_tokens: usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(Value::as_u64),
    })
}

/// Call the Google Gemini REST API (`generateContent`). Cost is left to the caller (compute from the
/// price book by tokens); the API doesn't return a dollar cost. Key from GEMINI_API_KEY (or GOOGLE_*).
fn generate_gemini(model: &str, system_prompt: Option<&str>, input: &str) -> Result<GenOutcome> {
    let key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .or_else(|_| std::env::var("GOOGLE_GENERATIVE_AI_API_KEY"))
        .map_err(|_| EngineError::Other("no Gemini API key (set GEMINI_API_KEY)".into()))?;
    let url =
        format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let mut body = serde_json::json!({
        "contents": [{ "role": "user", "parts": [{ "text": input }] }]
    });
    if let Some(sys) = system_prompt {
        body["system_instruction"] = serde_json::json!({ "parts": [{ "text": sys }] });
    }

    let client = reqwest::blocking::Client::new();
    let started = Instant::now();
    let resp = client
        .post(&url)
        .header("x-goog-api-key", &key)
        .json(&body)
        .send()
        .map_err(|e| EngineError::Other(format!("gemini request failed: {e}")))?;
    let latency_ms = Some(started.elapsed().as_millis() as u64);
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| EngineError::Other(format!("gemini read failed: {e}")))?;
    if !status.is_success() {
        return Err(EngineError::Other(format!(
            "gemini HTTP {}: {text}",
            status.as_u16()
        )));
    }
    let v: Value = serde_json::from_str(&text)?;
    let output = v
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.get(0))
        .and_then(|p| p.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let usage = v.get("usageMetadata");
    Ok(GenOutcome {
        output,
        cost_usd: None, // computed by the caller from the price book (Gemini API returns no $)
        model: model.to_string(),
        latency_ms,
        input_tokens: usage.and_then(|u| u.get("promptTokenCount")).and_then(Value::as_u64),
        output_tokens: usage
            .and_then(|u| u.get("candidatesTokenCount"))
            .and_then(Value::as_u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_from_judge_text() {
        // Judge output is now parsed from the model's text (provider-agnostic).
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
        assert_eq!(
            parse_judge_spec("openai/gpt-4o-mini"),
            ("openai".into(), "gpt-4o-mini".into())
        );
    }
}
