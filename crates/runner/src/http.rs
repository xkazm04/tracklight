//! Thin HTTP client over the LightTrack API.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::cli::Cli;

/// GET `path` and decode JSON into `T` (bearer auth if a key is set).
pub(crate) fn get<T: serde::de::DeserializeOwned>(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    path: &str,
) -> Result<T> {
    let mut req = http.get(format!("{}{}", cli.base, path));
    if let Some(k) = &cli.key {
        req = req.bearer_auth(k);
    }
    let resp = req.send()?;
    let status = resp.status();
    let text = resp.text()?;
    if !status.is_success() {
        anyhow::bail!("GET {path} -> HTTP {}: {text}", status.as_u16());
    }
    serde_json::from_str(&text).with_context(|| format!("decoding response from {path}"))
}

/// POST `body` to `path`, returning the JSON response (or Null).
pub(crate) fn post(
    cli: &Cli,
    http: &reqwest::blocking::Client,
    path: &str,
    body: &Value,
) -> Result<Value> {
    let mut req = http.post(format!("{}{}", cli.base, path)).json(body);
    if let Some(k) = &cli.key {
        req = req.bearer_auth(k);
    }
    let resp = req.send()?;
    let status = resp.status();
    let text = resp.text()?;
    if !status.is_success() {
        anyhow::bail!("POST {path} -> HTTP {}: {text}", status.as_u16());
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}
