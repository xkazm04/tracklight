//! Billing-provider webhook ingest (Stripe/Polar).
//!
//! Deliberately **unauthenticated** in the bearer-key sense — the provider's HMAC signature *is* the
//! auth, verified by the configured [`lighttrack_billing::BillingSource`]. The LightTrack project is
//! taken from `?project=` on the webhook URL (configure one endpoint per project in the provider).

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;

use crate::error::ApiError;
use crate::state::{spawn_db, AppState};

#[derive(Deserialize)]
pub(crate) struct WebhookParams {
    project: Option<String>,
}

pub(crate) async fn post_webhook(
    State(st): State<AppState>,
    Path(provider): Path<String>,
    Query(q): Query<WebhookParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let source = st
        .billing
        .get(&provider)
        .ok_or_else(|| ApiError::not_found(format!("billing provider '{provider}' is not configured")))?;
    let project = q
        .project
        .ok_or_else(|| ApiError::bad_request("webhook URL must include ?project=<id>"))?;

    let now = chrono::Utc::now().timestamp();
    let lookup = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut events = source
        .verify_webhook(&lookup, &body, now)
        .map_err(|e| ApiError::unauthorized(e.to_string()))?;
    for ev in &mut events {
        ev.project_id = project.clone();
    }

    let store = st.store.clone();
    let n = events.len();
    spawn_db(move || {
        for ev in &events {
            store.insert_revenue_event(ev)?;
        }
        Ok(())
    })
    .await?;
    eprintln!("[BILLING] {provider} webhook: stored {n} revenue record(s) for project={project}");
    Ok(StatusCode::OK)
}
