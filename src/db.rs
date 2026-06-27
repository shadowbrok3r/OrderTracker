//! SurrealDB connection singleton (server-only).
//! Set SURREAL_URL in env (e.g. ws://127.0.0.1:8000) and call ensure_db_init() before querying.

use std::sync::LazyLock;
use surrealdb::engine::remote::ws::{Client, Ws, Wss};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;
use surrealdb_types::SurrealValue;

const NS: &str = "jewelry_calculator";
const DB_NAME: &str = "jewelry_calculator";

/// Singleton DB; connect with ensure_db_init() at startup when SURREAL_URL is set.
pub static DB: LazyLock<Surreal<Client>> = LazyLock::new(Surreal::init);

static DB_INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Connect the singleton DB exactly once. Safe to call repeatedly (subsequent calls are no-ops).
pub async fn ensure_db_init() -> Result<(), String> {
    DB_INIT
        .get_or_try_init(|| async {
            let url = std::env::var("SURREAL_URL")
                .map_err(|_| "SURREAL_URL not set".to_string())?;
            let url = url.trim().to_string();
            if url.is_empty() {
                return Err("SURREAL_URL is empty".to_string());
            }
            // Typed Ws/Wss engines prepend the scheme; pass host only, not the full URL.
            let connect_result = if let Some(host) = url.strip_prefix("wss://") {
                DB.connect::<Wss>(host).await
            } else if let Some(host) = url.strip_prefix("ws://") {
                DB.connect::<Ws>(host).await
            } else {
                DB.connect::<Ws>(url.as_str()).await
            };
            match &connect_result {
                Ok(_) => eprintln!("Connected to SurrealDB at {}", url),
                Err(e) => eprintln!("Failed connecting to {}: {:?}", url, e),
            }
            connect_result.map_err(|e| e.to_string())?;
            if let (Ok(user), Ok(pass)) =
                (std::env::var("SURREAL_USER"), std::env::var("SURREAL_PASS"))
            {
                DB.signin(Root { username: user.clone(), password: pass })
                    .await
                    .map_err(|e| e.to_string())?;
                eprintln!("Signed in to SurrealDB as {}", user);
            }
            DB.use_ns(NS).use_db(DB_NAME).await.map_err(|e| e.to_string())?;
            eprintln!("Using NS: {}, DB: {}", NS, DB_NAME);
            Ok(())
        })
        .await
        .map(|_| ())
}

#[derive(SurrealValue)]
struct OrderCacheRow {
    source: String,
    order_number: String,
    payload: String,
}

/// JSON payloads of orders cached within the last day (empty = stale/miss).
pub async fn load_cached_orders() -> Result<Vec<String>, String> {
    let mut res = DB
        .query("SELECT VALUE payload FROM orders WHERE fetched_at > time::now() - 1d")
        .await
        .map_err(|e| e.to_string())?;
    res.take(0).map_err(|e| e.to_string())
}

/// Replace the order cache with the given set (fetched_at set to now).
pub async fn save_orders(orders: &[crate::model::Order]) -> Result<(), String> {
    let rows: Vec<OrderCacheRow> = orders
        .iter()
        .map(|o| OrderCacheRow {
            source: match o.source {
                crate::model::OrderSource::Shopify => "shopify".to_string(),
                crate::model::OrderSource::Etsy => "etsy".to_string(),
            },
            order_number: o.order_number.clone(),
            payload: serde_json::to_string(o).unwrap_or_default(),
        })
        .collect();
    DB.query("DELETE orders; INSERT INTO orders $rows")
        .bind(("rows", rows))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Load the catalog: every jewelry piece with its linked piece_costs sizes.
pub async fn load_catalog() -> Result<Vec<crate::model::CatalogPiece>, String> {
    let q = "SELECT name, kind, product_keys, \
        (SELECT ring_size, volume_cm3, silver_g, silver_usd, gold_g, gold_usd, bronze_g, bronze_usd, wax_usd \
         FROM piece_costs WHERE design_key = $parent.id) AS sizes \
        FROM jewelry ORDER BY name";
    let mut res = DB.query(q).await.map_err(|e| e.to_string())?;
    let pieces: Vec<crate::model::CatalogPiece> = res.take(0).map_err(|e| e.to_string())?;
    Ok(pieces)
}
