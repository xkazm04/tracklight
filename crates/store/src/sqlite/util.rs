//! Shared SQLite helpers: timestamp formatting/parsing, enum (de)serialization, JSON columns.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::{Result, StoreError};

/// Fixed-width, UTC, nanosecond RFC3339 (e.g. `2026-05-31T00:07:14.110948400Z`). Fixed width =>
/// lexicographic ordering matches chronological ordering, so `ts` range filters / `ORDER BY` are
/// correct as plain string comparisons.
pub(super) fn fmt_ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

pub(super) fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .map_err(|e| StoreError::Other(format!("bad ts {s:?}: {e}")))?
        .with_timezone(&Utc))
}

/// Serialize a string-valued enum to its on-disk string (e.g. `LimitMetric::CostUsd` -> "cost_usd").
pub(super) fn enum_to_str<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_value(v)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| StoreError::Other("enum did not serialize to a string".into()))
}

/// Parse a stored enum string, falling back to the type's default on any mismatch.
pub(super) fn parse_enum<T: DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_value(Value::String(s.to_string())).unwrap_or_default()
}

/// Serialize a JSON value to a column string, or `None` if it's `Null`.
pub(super) fn json_or_null(v: &Value) -> Result<Option<String>> {
    if v.is_null() {
        Ok(None)
    } else {
        Ok(Some(serde_json::to_string(v)?))
    }
}

/// Parse an optional column string back into a JSON value (`Null` if absent).
pub(super) fn val_or_null(s: Option<String>) -> Result<Value> {
    match s {
        Some(x) => Ok(serde_json::from_str(&x)?),
        None => Ok(Value::Null),
    }
}
