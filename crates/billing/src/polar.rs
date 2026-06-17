//! Polar webhook verification + normalization (the **Standard Webhooks** scheme Polar uses).
//!
//! Signature: three headers — `webhook-id`, `webhook-timestamp`, `webhook-signature`. The signed
//! content is `"{id}.{timestamp}.{body}"`, HMAC-SHA256 then **base64** (not hex). The
//! `webhook-signature` header is a space-separated list of `v1,<base64>` entries (key rotation); a
//! match on any is accepted, in constant time. Tolerance is 5 minutes.
//!
//! Key note: Polar's `validateEvent` base64-*encodes* the configured secret before handing it to the
//! Standard-Webhooks verifier (which base64-*decodes* it) — so the effective HMAC key is just the raw
//! bytes of `POLAR_WEBHOOK_SECRET`, verbatim. We use them directly. (Verified against
//! `@polar-sh/sdk` + `standardwebhooks` in the user's sandbox projects.)
//!
//! Revenue events: `order.paid` is the authoritative paid signal (Polar fires `order.created` before
//! capture); `order.refunded` / `refund.created` are clawbacks. Subscription cycles each arrive as a
//! fresh `order.paid` with their own id, so renewals re-recognize.

use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use lighttrack_core::{RevenueEvent, RevenueKind};

use crate::{BillingError, BillingSource};

type HmacSha256 = Hmac<Sha256>;

const TOLERANCE_SECS: i64 = 300;

pub struct PolarSource {
    secret: String,
    /// Order-metadata key to read the customer id from before falling back to Polar's `customer_id`.
    /// Defaults to `userId`: the apps echo their internal user id into Polar order metadata, and LLM
    /// events are tagged with that same id — so per-customer margin joins the two streams on it.
    customer_meta_key: String,
}

impl PolarSource {
    pub fn new(secret: impl Into<String>) -> Self {
        Self::with_customer_key(secret, "userId")
    }

    pub fn with_customer_key(secret: impl Into<String>, customer_meta_key: impl Into<String>) -> Self {
        Self { secret: secret.into(), customer_meta_key: customer_meta_key.into() }
    }
}

impl BillingSource for PolarSource {
    fn provider(&self) -> &'static str {
        "polar"
    }

    fn verify_webhook(
        &self,
        header: &dyn Fn(&str) -> Option<String>,
        body: &[u8],
        now_unix: i64,
    ) -> Result<Vec<RevenueEvent>, BillingError> {
        let id = header("webhook-id")
            .ok_or_else(|| BillingError::Signature("missing webhook-id".into()))?;
        let ts = header("webhook-timestamp")
            .ok_or_else(|| BillingError::Signature("missing webhook-timestamp".into()))?;
        let sig = header("webhook-signature")
            .ok_or_else(|| BillingError::Signature("missing webhook-signature".into()))?;
        verify_signature(&self.secret, &id, &ts, &sig, body, now_unix)?;
        let envelope: Value =
            serde_json::from_slice(body).map_err(|e| BillingError::Parse(e.to_string()))?;
        Ok(normalize(&envelope, &self.customer_meta_key))
    }
}

fn verify_signature(
    secret: &str,
    id: &str,
    timestamp: &str,
    sig_header: &str,
    body: &[u8],
    now_unix: i64,
) -> Result<(), BillingError> {
    let ts: i64 = timestamp.parse().map_err(|_| BillingError::Signature("bad timestamp".into()))?;
    if (now_unix - ts).abs() > TOLERANCE_SECS {
        return Err(BillingError::Signature("timestamp outside tolerance".into()));
    }
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| BillingError::Signature("bad signing key".into()))?;
    mac.update(format!("{id}.{ts}.").as_bytes());
    mac.update(body);
    let expected = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    // `webhook-signature` is space-separated `version,signature`; accept any matching `v1`.
    let matched = sig_header.split(' ').any(|entry| {
        matches!(entry.split_once(','), Some(("v1", sig)) if ct_eq(sig.as_bytes(), expected.as_bytes()))
    });
    if matched {
        Ok(())
    } else {
        Err(BillingError::Signature("no matching signature".into()))
    }
}

/// Normalize a Polar event `{type, data}` into revenue records (empty for events we don't track).
/// `customer_meta_key` is the order-metadata field to key the customer on (else Polar `customer_id`).
pub fn normalize(envelope: &Value, customer_meta_key: &str) -> Vec<RevenueEvent> {
    let typ = envelope.get("type").and_then(Value::as_str).unwrap_or("");
    let null = Value::Null;
    let data = envelope.get("data").unwrap_or(&null);
    match typ {
        "order.paid" => normalize_order(data, customer_meta_key).into_iter().collect(),
        "order.refunded" => normalize_order_refund(data, customer_meta_key).into_iter().collect(),
        "refund.created" => normalize_refund(data, customer_meta_key).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// A paid Polar order → a (subscription or one-time) revenue record.
pub fn normalize_order(obj: &Value, customer_meta_key: &str) -> Option<RevenueEvent> {
    let id = obj.get("id").and_then(Value::as_str)?;
    let amount = amount_minor(obj)?;
    let subscription = obj.get("subscription_id").and_then(Value::as_str);
    let sub_obj = obj.get("subscription");
    let (period_start, period_end) = sub_obj
        .map(|s| (rfc_dt(s.get("current_period_start")), rfc_dt(s.get("current_period_end"))))
        .unwrap_or((None, None));
    Some(RevenueEvent {
        id: format!("polar:{id}"),
        project_id: String::new(),
        source: "polar".into(),
        external_id: Some(id.to_string()),
        customer_id: customer_id(obj, customer_meta_key),
        product_id: product_id(obj),
        amount_usd: crate::to_major(amount),
        currency: currency(obj),
        kind: if subscription.is_some() {
            RevenueKind::Subscription
        } else {
            RevenueKind::OneTime
        },
        period_start,
        period_end,
        ts: rfc_dt(obj.get("created_at")).unwrap_or_else(Utc::now),
    })
}

/// An `order.refunded` event (data is an Order with `refunded_amount`) → a refund record.
pub fn normalize_order_refund(obj: &Value, customer_meta_key: &str) -> Option<RevenueEvent> {
    let order_id = obj.get("order_id").and_then(Value::as_str)
        .or_else(|| obj.get("id").and_then(Value::as_str))?;
    let amount = obj.get("refunded_amount").and_then(Value::as_i64).or_else(|| amount_minor(obj))?;
    if amount == 0 {
        return None;
    }
    Some(refund_event(order_id, amount, obj, customer_meta_key))
}

/// A `refund.created` event (data is a Refund) → a refund record.
pub fn normalize_refund(obj: &Value, customer_meta_key: &str) -> Option<RevenueEvent> {
    let id = obj.get("id").and_then(Value::as_str)?;
    let amount = obj.get("amount").and_then(Value::as_i64)?;
    if amount == 0 {
        return None;
    }
    Some(refund_event(id, amount, obj, customer_meta_key))
}

fn refund_event(key: &str, amount_minor: i64, obj: &Value, customer_meta_key: &str) -> RevenueEvent {
    RevenueEvent {
        id: format!("polar:refund:{key}"),
        project_id: String::new(),
        source: "polar".into(),
        external_id: Some(format!("refund:{key}")),
        customer_id: customer_id(obj, customer_meta_key),
        product_id: None,
        amount_usd: crate::to_major(amount_minor),
        currency: currency(obj),
        kind: RevenueKind::Refund,
        period_start: None,
        period_end: None,
        ts: rfc_dt(obj.get("created_at")).unwrap_or_else(Utc::now),
    }
}

/// Amount in minor units: prefer what the customer paid (`total_amount`), else net/subtotal/amount.
fn amount_minor(obj: &Value) -> Option<i64> {
    ["total_amount", "net_amount", "subtotal_amount", "amount"]
        .into_iter()
        .find_map(|k| obj.get(k).and_then(Value::as_i64))
}

/// Customer id for margin attribution: prefer `metadata.<key>` (the app's userId, which LLM events
/// are also tagged with), then Polar's top-level `customer_id`, then the nested `customer.id`.
fn customer_id(obj: &Value, meta_key: &str) -> Option<String> {
    obj.pointer(&format!("/metadata/{meta_key}"))
        .and_then(Value::as_str)
        .or_else(|| obj.get("customer_id").and_then(Value::as_str))
        .or_else(|| obj.pointer("/customer/id").and_then(Value::as_str))
        .map(str::to_string)
}

fn product_id(obj: &Value) -> Option<String> {
    obj.get("product_id")
        .and_then(Value::as_str)
        .or_else(|| obj.pointer("/product/id").and_then(Value::as_str))
        .map(str::to_string)
}

fn currency(obj: &Value) -> String {
    obj.get("currency").and_then(Value::as_str).unwrap_or("usd").to_uppercase()
}

fn rfc_dt(v: Option<&Value>) -> Option<DateTime<Utc>> {
    v.and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// Constant-time byte-slice equality (lengths are public — base64 of a 32-byte HMAC is fixed-width).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Sign exactly as Polar (Standard Webhooks) does: HMAC over `{id}.{ts}.{body}` with the raw
    /// secret bytes as key, base64-encoded, prefixed `v1,`.
    fn sign(secret: &str, id: &str, ts: i64, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(format!("{id}.{ts}.").as_bytes());
        mac.update(body);
        format!("v1,{}", base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
    }

    fn lookup(id: &str, ts: i64, sig: &str) -> impl Fn(&str) -> Option<String> {
        let (id, sig) = (id.to_string(), sig.to_string());
        move |name: &str| match name.to_ascii_lowercase().as_str() {
            "webhook-id" => Some(id.clone()),
            "webhook-timestamp" => Some(ts.to_string()),
            "webhook-signature" => Some(sig.clone()),
            _ => None,
        }
    }

    fn order_paid() -> Vec<u8> {
        // Mirrors a real Polar sandbox order.paid (from the user's webhook itest fixtures).
        serde_json::to_vec(&json!({
            "type": "order.paid",
            "timestamp": "2026-06-12T10:00:00Z",
            "data": {
                "id": "ord_1", "created_at": "2026-06-12T10:00:00Z", "status": "paid", "paid": true,
                "subtotal_amount": 2000, "net_amount": 2000, "total_amount": 2000, "tax_amount": 0,
                "currency": "usd", "customer_id": "cust_1", "product_id": "prod_pro",
                "subscription_id": "sub_9",
                "subscription": {
                    "current_period_start": "2026-06-12T10:00:00Z",
                    "current_period_end": "2026-07-12T10:00:00Z"
                },
                "metadata": { "userId": "u-1" }
            }
        }))
        .unwrap()
    }

    #[test]
    fn valid_signature_parses_order() {
        let secret = "polar_whsec_sandbox";
        let body = order_paid();
        let now = 1_780_000_000_i64;
        let sig = sign(secret, "wh_1", now, &body);

        let events = PolarSource::new(secret)
            .verify_webhook(&lookup("wh_1", now, &sig), &body, now)
            .unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.id, "polar:ord_1");
        // metadata.userId wins over the Polar customer_id — it's the LLM-event join key.
        assert_eq!(e.customer_id.as_deref(), Some("u-1"));
        assert_eq!(e.product_id.as_deref(), Some("prod_pro"));
        assert!((e.amount_usd - 20.0).abs() < 1e-9);
        assert_eq!(e.kind, RevenueKind::Subscription);
        assert!(e.period_start.is_some() && e.period_end.is_some());
    }

    #[test]
    fn tampered_body_is_rejected() {
        let secret = "polar_whsec_sandbox";
        let body = order_paid();
        let now = 1_780_000_000_i64;
        let sig = sign(secret, "wh_1", now, &body);
        let mut tampered = body.clone();
        tampered[10] ^= 0x01;
        assert!(PolarSource::new(secret)
            .verify_webhook(&lookup("wh_1", now, &sig), &tampered, now)
            .is_err());
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let body = order_paid();
        let now = 1_780_000_000_i64;
        let sig = sign("polar_whsec_sandbox", "wh_1", now, &body);
        assert!(PolarSource::new("polar_whsec_other")
            .verify_webhook(&lookup("wh_1", now, &sig), &body, now)
            .is_err());
    }

    #[test]
    fn stale_timestamp_is_rejected() {
        let secret = "polar_whsec_sandbox";
        let body = order_paid();
        let signed_at = 1_780_000_000_i64;
        let sig = sign(secret, "wh_1", signed_at, &body);
        assert!(PolarSource::new(secret)
            .verify_webhook(&lookup("wh_1", signed_at, &sig), &body, signed_at + 3600)
            .is_err());
    }

    #[test]
    fn customer_id_prefers_metadata_then_falls_back() {
        // metadata.userId present → it wins (LLM events are tagged with the same userId).
        let with_meta = json!({ "id": "o1", "total_amount": 1000, "currency": "usd",
            "customer_id": "cust_x", "metadata": { "userId": "user-42" } });
        assert_eq!(
            normalize_order(&with_meta, "userId").unwrap().customer_id.as_deref(),
            Some("user-42")
        );
        // no metadata.userId → fall back to Polar's customer_id.
        let no_meta = json!({ "id": "o2", "total_amount": 1000, "currency": "usd", "customer_id": "cust_y" });
        assert_eq!(
            normalize_order(&no_meta, "userId").unwrap().customer_id.as_deref(),
            Some("cust_y")
        );
    }

    #[test]
    fn order_refund_normalizes_negative() {
        let r = normalize(
            &json!({
                "type": "order.refunded",
                "data": { "id": "ord_1", "order_id": "ord_1", "refunded_amount": 500,
                          "currency": "usd", "customer_id": "cust_1", "created_at": "2026-06-13T00:00:00Z" }
            }),
            "userId",
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, RevenueKind::Refund);
        assert!((r[0].amount_usd - 5.0).abs() < 1e-9);
        assert_eq!(r[0].id, "polar:refund:ord_1");
    }

    #[test]
    fn untracked_event_is_ignored() {
        assert!(normalize(&json!({ "type": "checkout.updated", "data": {} }), "userId").is_empty());
    }
}
