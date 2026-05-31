//! Candidate-output generation across providers. `anthropic` runs via `claude -p`; `google` and
//! `openai` call their HTTPS APIs (keys from env). Dollar cost is left `None` for the HTTP providers
//! (the caller prices it from the DB price book by tokens); the APIs don't return a cost.

use std::time::Instant;

use serde_json::Value;

use crate::{claude, EngineConfig, EngineError, GenOutcome, Result};

/// Generate a candidate output from a target (provider + model + optional system-prompt variant).
pub fn generate(
    cfg: &EngineConfig,
    provider: &str,
    model: &str,
    system_prompt: Option<&str>,
    input: &str,
) -> Result<GenOutcome> {
    match provider {
        "anthropic" => {
            let (envelope, latency_ms) = claude::invoke(cfg, input, model, system_prompt, None)?;
            let (input_tokens, output_tokens) = claude::token_counts(&envelope);
            Ok(GenOutcome {
                output: envelope
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                cost_usd: envelope.get("total_cost_usd").and_then(Value::as_f64),
                model: claude::model_of(&envelope, model),
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

/// Google Gemini `generateContent`. Key from GEMINI_API_KEY (or GOOGLE_* fallbacks).
fn generate_gemini(model: &str, system_prompt: Option<&str>, input: &str) -> Result<GenOutcome> {
    let key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .or_else(|_| std::env::var("GOOGLE_GENERATIVE_AI_API_KEY"))
        .map_err(|_| EngineError::Other("no Gemini API key (set GEMINI_API_KEY)".into()))?;
    let url =
        format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent");
    let mut body = serde_json::json!({ "contents": [{ "role": "user", "parts": [{ "text": input }] }] });
    if let Some(sys) = system_prompt {
        body["system_instruction"] = serde_json::json!({ "parts": [{ "text": sys }] });
    }

    let started = Instant::now();
    let resp = reqwest::blocking::Client::new()
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
        return Err(EngineError::Other(format!("gemini HTTP {}: {text}", status.as_u16())));
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
        cost_usd: None,
        model: model.to_string(),
        latency_ms,
        input_tokens: usage.and_then(|u| u.get("promptTokenCount")).and_then(Value::as_u64),
        output_tokens: usage
            .and_then(|u| u.get("candidatesTokenCount"))
            .and_then(Value::as_u64),
    })
}

/// OpenAI Chat Completions. Key from OPENAI_API_KEY.
fn generate_openai(model: &str, system_prompt: Option<&str>, input: &str) -> Result<GenOutcome> {
    let key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| EngineError::Other("no OpenAI API key (set OPENAI_API_KEY)".into()))?;
    let mut messages = Vec::new();
    if let Some(sys) = system_prompt {
        messages.push(serde_json::json!({ "role": "system", "content": sys }));
    }
    messages.push(serde_json::json!({ "role": "user", "content": input }));
    let body = serde_json::json!({ "model": model, "messages": messages });

    let started = Instant::now();
    let resp = reqwest::blocking::Client::new()
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
        return Err(EngineError::Other(format!("openai HTTP {}: {text}", status.as_u16())));
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
        cost_usd: None,
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
