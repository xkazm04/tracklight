//! Model prices (Phase 3.6a) — DB-backed, hot-swappable.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use serde::Deserialize;

use lighttrack_core::{ModelPriceRow, PriceBook};

use crate::error::ApiError;
use crate::guards::{authenticate, ensure_can_admin};
use crate::state::{spawn_db, AppState};

pub(crate) async fn get_prices(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ModelPriceRow>>, ApiError> {
    authenticate(&st, &headers).await?;
    let store = st.store.clone();
    let rows = spawn_db(move || store.list_prices()).await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub(crate) struct PutPriceReq {
    input_per_mtok: f64,
    output_per_mtok: f64,
    #[serde(default)]
    cached_input_per_mtok: Option<f64>,
    #[serde(default)]
    source_url: Option<String>,
}

pub(crate) async fn put_price(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path((provider, model)): Path<(String, String)>,
    Json(req): Json<PutPriceReq>,
) -> Result<Json<ModelPriceRow>, ApiError> {
    ensure_can_admin(&authenticate(&st, &headers).await?)?;
    let row = ModelPriceRow {
        provider,
        model,
        input_per_mtok: req.input_per_mtok,
        output_per_mtok: req.output_per_mtok,
        cached_input_per_mtok: req.cached_input_per_mtok,
        effective_date: Utc::now(),
        source_url: req.source_url,
    };
    let store = st.store.clone();
    let row2 = row.clone();
    spawn_db(move || store.upsert_price(&row2)).await?;

    // Hot-swap the in-memory price book so new prices take effect without a restart.
    let store2 = st.store.clone();
    let rows = spawn_db(move || store2.list_prices()).await?;
    {
        let mut book = st.prices.write().unwrap();
        *book = PriceBook::from_rows(&rows);
    }
    Ok(Json(row))
}
