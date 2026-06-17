//! Stripe webhook verification + normalization.
//!
//! Signature scheme (Stripe "Signing secret"): the `Stripe-Signature` header is `t=<unix>,v1=<hex>`,
//! where `<hex>` is HMAC-SHA256 of `"{t}.{body}"` keyed by the signing secret. We verify in
//! constant time and bound replay by a timestamp tolerance.
//!
//! Amount note (Phase 2): Stripe amounts are in the currency's minor unit; we divide by 100 and keep
//! the currency label. Zero-decimal currencies (e.g. JPY) and FX→USD are a documented follow-up.

use chrono::{DateTime, TimeZone, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use lighttrack_core::{RevenueEvent, RevenueKind};

use crate::{BillingError, BillingSource};

type HmacSha256 = Hmac<Sha256>;

/// Max accepted skew between the webhook timestamp and now, to bound replay.
const TOLERANCE_SECS: i64 = 300;

pub struct StripeSource {
    secret: String,
}

impl StripeSource {
    pub fn new(secret: impl Into<String>) -> Self {
        Self { secret: secret.into() }
    }
}

impl BillingSource for StripeSource {
    fn provider(&self) -> &'static str {
        "stripe"
    }

    fn verify_webhook(
        &self,
        header: &dyn Fn(&str) -> Option<String>,
        body: &[u8],
        now_unix: i64,
    ) -> Result<Vec<RevenueEvent>, BillingError> {
        let sig = header("Stripe-Signature")
            .ok_or_else(|| BillingError::Signature("missing Stripe-Signature header".into()))?;
        verify_signature(&self.secret, &sig, body, now_unix)?;
        let envelope: Value =
            serde_json::from_slice(body).map_err(|e| BillingError::Parse(e.to_string()))?;
        Ok(normalize(&envelope))
    }
}

/// Verify Stripe's `t=…,v1=…` signature header.
fn verify_signature(secret: &str, header: &str, body: &[u8], now_unix: i64) -> Result<(), BillingError> {
    let (mut t, mut v1) = (None, None);
    for part in header.split(',') {
        if let Some((k, val)) = part.split_once('=') {
            match k.trim() {
                "t" => t = Some(val.trim()),
                "v1" => v1 = Some(val.trim()),
                _ => {}
            }
        }
    }
    let t = t.ok_or_else(|| BillingError::Signature("missing timestamp".into()))?;
    let v1 = v1.ok_or_else(|| BillingError::Signature("missing v1 signature".into()))?;
    let ts: i64 = t.parse().map_err(|_| BillingError::Signature("bad timestamp".into()))?;
    if (now_unix - ts).abs() > TOLERANCE_SECS {
        return Err(BillingError::Signature("timestamp outside tolerance".into()));
    }
    let expected = decode_hex(v1).ok_or_else(|| BillingError::Signature("bad hex signature".into()))?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| BillingError::Signature("bad signing key".into()))?;
    mac.update(t.as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.verify_slice(&expected)
        .map_err(|_| BillingError::Signature("signature mismatch".into()))
}

/// Normalize a Stripe event envelope `{type, data:{object}}` into revenue records. Events we don't
/// track yield an empty vec.
pub fn normalize(envelope: &Value) -> Vec<RevenueEvent> {
    let typ = envelope.get("type").and_then(Value::as_str).unwrap_or("");
    let null = Value::Null;
    let obj = envelope.pointer("/data/object").unwrap_or(&null);
    match typ {
        "invoice.paid" | "invoice.payment_succeeded" => normalize_invoice(obj).into_iter().collect(),
        "charge.refunded" => normalize_refund(obj).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// A paid Stripe invoice → a (subscription or one-time) revenue record. `project_id` is left blank;
/// the webhook handler / sync caller stamps it. `id` is deterministic for idempotent upsert.
pub fn normalize_invoice(obj: &Value) -> Option<RevenueEvent> {
    let id = obj.get("id").and_then(Value::as_str)?;
    let amount_cents = obj
        .get("amount_paid")
        .and_then(Value::as_i64)
        .or_else(|| obj.get("amount_due").and_then(Value::as_i64))?;
    let currency = obj.get("currency").and_then(Value::as_str).unwrap_or("usd");
    let line = obj.pointer("/lines/data/0");
    let (period_start, period_end) = line
        .and_then(|l| l.get("period"))
        .map(|p| (unix_dt(p.get("start")), unix_dt(p.get("end"))))
        .unwrap_or((None, None));
    let kind = if obj.get("subscription").and_then(Value::as_str).is_some() {
        RevenueKind::Subscription
    } else {
        RevenueKind::OneTime
    };
    Some(RevenueEvent {
        id: format!("stripe:{id}"),
        project_id: String::new(),
        source: "stripe".into(),
        external_id: Some(id.to_string()),
        customer_id: obj.get("customer").and_then(Value::as_str).map(str::to_string),
        product_id: line
            .and_then(|l| l.pointer("/price/product"))
            .and_then(Value::as_str)
            .map(str::to_string),
        amount_usd: crate::to_major(amount_cents),
        currency: currency.to_uppercase(),
        kind,
        period_start,
        period_end,
        ts: unix_dt(obj.get("created")).unwrap_or_else(Utc::now),
    })
}

/// A refunded Stripe charge → a negative (refund) revenue record.
pub fn normalize_refund(obj: &Value) -> Option<RevenueEvent> {
    let id = obj.get("id").and_then(Value::as_str)?;
    let refunded = obj.get("amount_refunded").and_then(Value::as_i64)?;
    if refunded == 0 {
        return None;
    }
    let currency = obj.get("currency").and_then(Value::as_str).unwrap_or("usd");
    Some(RevenueEvent {
        id: format!("stripe:refund:{id}"),
        project_id: String::new(),
        source: "stripe".into(),
        external_id: Some(format!("refund:{id}")),
        customer_id: obj.get("customer").and_then(Value::as_str).map(str::to_string),
        product_id: None,
        amount_usd: crate::to_major(refunded),
        currency: currency.to_uppercase(),
        kind: RevenueKind::Refund,
        period_start: None,
        period_end: None,
        ts: unix_dt(obj.get("created")).unwrap_or_else(Utc::now),
    })
}

fn unix_dt(v: Option<&Value>) -> Option<DateTime<Utc>> {
    v.and_then(Value::as_i64)
        .and_then(|s| Utc.timestamp_opt(s, 0).single())
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encode_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A header-lookup closure returning `header` for `Stripe-Signature` (case-insensitive).
    fn lookup(header: &str) -> impl Fn(&str) -> Option<String> {
        let header = header.to_string();
        move |name: &str| name.eq_ignore_ascii_case("stripe-signature").then(|| header.clone())
    }

    /// Produce a valid `Stripe-Signature` header for `body` at time `t` with `secret`.
    fn sign(secret: &str, t: i64, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(t.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        format!("t={t},v1={}", encode_hex(&mac.finalize().into_bytes()))
    }

    fn invoice_envelope() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "type": "invoice.paid",
            "data": { "object": {
                "id": "in_123", "customer": "cus_42", "subscription": "sub_9",
                "amount_paid": 2000, "currency": "usd", "created": 1_780_000_000_i64,
                "lines": { "data": [ {
                    "period": { "start": 1_780_000_000_i64, "end": 1_782_592_000_i64 },
                    "price": { "product": "prod_chat" }
                } ] }
            } }
        }))
        .unwrap()
    }

    #[test]
    fn valid_signature_parses_invoice() {
        let secret = "whsec_test";
        let body = invoice_envelope();
        let now = 1_780_000_100_i64;
        let header = sign(secret, now, &body);
        let src = StripeSource::new(secret);

        let events = src.verify_webhook(&lookup(&header), &body, now).unwrap();
        assert_eq!(events.len(), 1);
        let e = &events[0];
        assert_eq!(e.id, "stripe:in_123");
        assert_eq!(e.external_id.as_deref(), Some("in_123"));
        assert_eq!(e.customer_id.as_deref(), Some("cus_42"));
        assert_eq!(e.product_id.as_deref(), Some("prod_chat"));
        assert!((e.amount_usd - 20.0).abs() < 1e-9);
        assert_eq!(e.kind, RevenueKind::Subscription);
        assert!(e.period_start.is_some() && e.period_end.is_some());
    }

    #[test]
    fn tampered_body_is_rejected() {
        let secret = "whsec_test";
        let body = invoice_envelope();
        let now = 1_780_000_100_i64;
        let header = sign(secret, now, &body);
        let mut tampered = body.clone();
        tampered[0] ^= 0x01;
        assert!(StripeSource::new(secret).verify_webhook(&lookup(&header), &tampered, now).is_err());
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let body = invoice_envelope();
        let now = 1_780_000_100_i64;
        let header = sign("whsec_test", now, &body);
        assert!(StripeSource::new("whsec_other").verify_webhook(&lookup(&header), &body, now).is_err());
    }

    #[test]
    fn stale_timestamp_is_rejected() {
        let secret = "whsec_test";
        let body = invoice_envelope();
        let signed_at = 1_780_000_000_i64;
        let header = sign(secret, signed_at, &body);
        // now is an hour later → outside the 5-minute tolerance
        assert!(StripeSource::new(secret)
            .verify_webhook(&lookup(&header), &body, signed_at + 3600)
            .is_err());
    }

    #[test]
    fn refund_normalizes_negative_kind() {
        let r = normalize(&json!({
            "type": "charge.refunded",
            "data": { "object": {
                "id": "ch_7", "customer": "cus_42", "amount_refunded": 500,
                "currency": "usd", "created": 1_780_000_000_i64
            } }
        }));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].kind, RevenueKind::Refund);
        assert!((r[0].amount_usd - 5.0).abs() < 1e-9);
        assert_eq!(r[0].id, "stripe:refund:ch_7");
    }

    #[test]
    fn untracked_event_is_ignored() {
        assert!(normalize(&json!({ "type": "customer.created", "data": { "object": {} } })).is_empty());
    }
}
