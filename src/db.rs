//! SurrealDB connection singleton (server-only).
//! Set SURREAL_URL in env (e.g. ws://127.0.0.1:8000) and call ensure_db_init() before querying.

use std::sync::LazyLock;
use surrealdb::engine::remote::ws::{Client, Ws, Wss};
use surrealdb::Surreal;

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
            let connect_result = if url.starts_with("wss") {
                DB.connect::<Wss>(&url).await
            } else {
                DB.connect::<Ws>(&url).await
            };
            match &connect_result {
                Ok(_) => eprintln!("Connected to SurrealDB at {}", url),
                Err(e) => eprintln!("Failed connecting to {}: {:?}", url, e),
            }
            connect_result.map_err(|e| e.to_string())?;
            DB.use_ns(NS).use_db(DB_NAME).await.map_err(|e| e.to_string())?;
            eprintln!("Using NS: {}, DB: {}", NS, DB_NAME);
            Ok(())
        })
        .await
        .map(|_| ())
}

/// Load all piece_costs from the database (call after ensure_db_init()).
pub async fn load_piece_costs() -> Result<Vec<crate::model::PieceCostRow>, String> {
    let rows: Vec<crate::model::PieceCostRow> = DB
        .select("piece_costs")
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows)
}
