//! The `claude -p` subprocess caller and shared envelope helpers.

use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::Value;

use crate::{EngineConfig, EngineError, Result};

/// Run `claude -p` with the given prompt/model, returning the parsed JSON envelope and latency.
pub(crate) fn invoke(
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

/// Total (input, output) tokens from a claude `usage` block (input includes cache read + creation).
pub(crate) fn token_counts(envelope: &Value) -> (Option<u64>, Option<u64>) {
    let usage = envelope.get("usage");
    let input = usage.map(|u| {
        let f = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        f("input_tokens") + f("cache_read_input_tokens") + f("cache_creation_input_tokens")
    });
    let output = usage.and_then(|u| u.get("output_tokens").and_then(Value::as_u64));
    (input, output)
}

/// Resolve the model name reported in the envelope, falling back to `fallback`.
pub(crate) fn model_of(envelope: &Value, fallback: &str) -> String {
    envelope
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}
