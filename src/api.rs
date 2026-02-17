//! Server functions bridging client UI to server-side API/DB logic.
//! These are callable from both web (WASM) and desktop clients.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::model::{Order, PieceCostRow};

/// Result of fetching orders from all sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchOrdersResult {
    pub orders: Vec<Order>,
    pub errors: Vec<String>,
}

/// Fetch orders from Shopify and Etsy. Errors from individual sources are
/// collected in `errors` so partial results are still returned.
#[server]
pub async fn fetch_all_orders() -> Result<FetchOrdersResult, ServerFnError> {
    let mut all_orders = Vec::new();
    let mut errors = Vec::new();

    match crate::shopify::fetch_shopify_orders().await {
        Ok(shopify_orders) => all_orders.extend(shopify_orders),
        Err(e) => errors.push(format!("Shopify: {}", e)),
    }

    match crate::etsy::fetch_etsy_orders().await {
        Ok(etsy_orders) => all_orders.extend(etsy_orders),
        Err(e) => errors.push(format!("Etsy: {}", e)),
    }

    all_orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));
    Ok(FetchOrdersResult {
        orders: all_orders,
        errors,
    })
}

/// Load piece costs from SurrealDB (initialises the DB connection on first call).
#[server]
pub async fn fetch_piece_costs() -> Result<Vec<PieceCostRow>, ServerFnError> {
    crate::db::ensure_db_init()
        .await
        .map_err(|e| ServerFnError::new(e))?;
    crate::db::load_piece_costs()
        .await
        .map_err(|e| ServerFnError::new(e))
}

/// Save an Etsy OAuth refresh token (persisted to disk on the server).
#[server]
pub async fn save_etsy_token(token: String) -> Result<(), ServerFnError> {
    crate::etsy::save_etsy_refresh_token(token)
        .map_err(|e| ServerFnError::new(e))
}
