//! Server functions bridging client UI to server-side API/DB logic.
//! These are callable from both web (WASM) and desktop clients.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::model::{CatalogPiece, Order};

/// Result of fetching orders from all sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchOrdersResult {
    pub orders: Vec<Order>,
    pub errors: Vec<String>,
}

/// Fetch live from Shopify + Etsy (errors collected per source) and replace the
/// SurrealDB order cache.
#[cfg(feature = "server")]
pub(crate) async fn live_fetch_and_cache() -> FetchOrdersResult {
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

    let mut orders = if crate::db::ensure_db_init().await.is_ok() {
        if !all_orders.is_empty() {
            cache_thumbnails(&mut all_orders).await;
            if let Err(e) = crate::db::save_orders(&all_orders).await {
                crate::log::app_log("INFO", format!("Order cache write failed: {}", e));
            }
        }
        crate::db::merge_orders(all_orders).await
    } else {
        all_orders
    };
    orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));
    FetchOrdersResult { orders, errors }
}

/// Download each order item's source image into the SurrealDB thumbnails bucket
/// and rewrite image_url to the app-served `thumb/<key>` path.
#[cfg(feature = "server")]
async fn cache_thumbnails(orders: &mut [Order]) {
    let client = reqwest::Client::new();
    for order in orders.iter_mut() {
        for item in order.items.iter_mut() {
            if let Some(url) = item.image_url.clone() {
                if url.starts_with("http") {
                    if let Some(cached) = crate::db::cache_thumbnail(&client, &url).await {
                        item.image_url = Some(cached);
                    }
                }
            }
        }
    }
}

/// Return cached orders if fresh (< 1 day), otherwise fetch live and cache.
/// Cache-aware order fetch (no HA push) shared by the server fn and the
/// background HA-refresh loop.
#[cfg(feature = "server")]
async fn fetch_orders_internal() -> FetchOrdersResult {
    if crate::db::ensure_db_init().await.is_ok() {
        if let Ok(base) = crate::db::load_cached_orders().await {
            if !base.is_empty() {
                let mut orders = crate::db::merge_orders(base).await;
                orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));
                crate::log::app_log("INFO", format!("Served {} orders from cache.", orders.len()));
                return FetchOrdersResult { orders, errors: Vec::new() };
            }
        }
    }
    live_fetch_and_cache().await
}

#[server]
pub async fn fetch_all_orders() -> Result<FetchOrdersResult, ServerFnError> {
    let result = fetch_orders_internal().await;
    crate::ha::push_orders(&result.orders).await;
    Ok(result)
}

/// Force a live Shopify + Etsy fetch and refresh the cache (manual Refresh).
#[server]
pub async fn refresh_orders() -> Result<FetchOrdersResult, ServerFnError> {
    let result = live_fetch_and_cache().await;
    crate::ha::push_orders(&result.orders).await;
    Ok(result)
}

/// Load the jewelry catalog (pieces + linked sizes) from SurrealDB.
#[server]
pub async fn fetch_catalog() -> Result<Vec<CatalogPiece>, ServerFnError> {
    crate::db::ensure_db_init()
        .await
        .map_err(|e| ServerFnError::new(e))?;
    crate::db::load_catalog()
        .await
        .map_err(|e| ServerFnError::new(e))
}

/// Save an Etsy OAuth refresh token (persisted to disk on the server).
#[server]
pub async fn save_etsy_token(token: String) -> Result<(), ServerFnError> {
    crate::etsy::save_etsy_refresh_token(token)
        .map_err(|e| ServerFnError::new(e))
}

/// Archive/complete an order (persisted overlay; key = Order::state_key).
#[server]
pub async fn set_order_state(key: String, archived: bool, completed: bool) -> Result<(), ServerFnError> {
    crate::db::ensure_db_init().await.map_err(|e| ServerFnError::new(e))?;
    crate::db::set_order_state(&key, archived, completed)
        .await
        .map_err(|e| ServerFnError::new(e))
}

/// Create a manual/custom order (not from Shopify/Etsy).
#[server]
pub async fn create_custom_order(order: Order) -> Result<(), ServerFnError> {
    crate::db::ensure_db_init().await.map_err(|e| ServerFnError::new(e))?;
    crate::db::save_custom_order(&order)
        .await
        .map_err(|e| ServerFnError::new(e))
}

/// Update an existing custom order (e.g. set the charge / line items).
#[server]
pub async fn update_custom_order(order: Order) -> Result<(), ServerFnError> {
    crate::db::ensure_db_init().await.map_err(|e| ServerFnError::new(e))?;
    crate::db::update_custom_order(&order)
        .await
        .map_err(|e| ServerFnError::new(e))
}

/// Link an Etsy/Shopify product name to a catalog piece (adds to product_keys).
#[server]
pub async fn link_product(piece_name: String, product_key: String) -> Result<(), ServerFnError> {
    crate::db::ensure_db_init().await.map_err(|e| ServerFnError::new(e))?;
    crate::db::link_product(&piece_name, &product_key)
        .await
        .map_err(|e| ServerFnError::new(e))
}

/// Save free-text production notes for an order (SurrealDB overlay).
#[server]
pub async fn set_order_notes(key: String, notes: String) -> Result<(), ServerFnError> {
    crate::db::ensure_db_init().await.map_err(|e| ServerFnError::new(e))?;
    crate::db::set_order_notes(&key, &notes)
        .await
        .map_err(|e| ServerFnError::new(e))
}

/// Set the production stage for an order (SurrealDB overlay).
#[server]
pub async fn set_order_stage(key: String, stage: String) -> Result<(), ServerFnError> {
    crate::db::ensure_db_init().await.map_err(|e| ServerFnError::new(e))?;
    crate::db::set_order_stage(&key, &stage)
        .await
        .map_err(|e| ServerFnError::new(e))
}
