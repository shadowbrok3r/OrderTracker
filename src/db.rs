//! SurrealDB connection and piece_costs lookup (shared with jewelry_cost_calculator).
//! Set SURREAL_URL in .env (e.g. ws://127.0.0.1:8000 or wss://...) and call init_db() at startup.

use std::sync::LazyLock;
use serde::Deserialize;
use surrealdb::engine::remote::ws::{Client, Ws, Wss};
use surrealdb::Surreal;
use surrealdb_types::SurrealValue;

use crate::model::{MetalType, OrderItem};

const NS: &str = "jewelry_calculator";
const DB_NAME: &str = "jewelry_calculator";
const SURREAL_URL: &str = env!("SURREAL_URL");

/// Singleton DB; connect with init_db() at startup when SURREAL_URL is set.
pub static DB: LazyLock<Surreal<Client>> = LazyLock::new(Surreal::init);

/// One row from piece_costs table.
#[derive(Debug, Clone, PartialEq, Deserialize, SurrealValue)]
pub struct PieceCostRow {
    pub design_key: String,
    pub ring_size: Option<String>,
    pub volume_cm3: Option<f64>,
    pub silver_g: Option<f64>,
    pub silver_usd: Option<f64>,
    pub gold_g: Option<f64>,
    pub gold_usd: Option<f64>,
    pub bronze_g: Option<f64>,
    pub bronze_usd: Option<f64>,
    pub wax_usd: Option<f64>,
    pub product_keys: Option<Vec<String>>,
}

/// Initialize the singleton DB (connect + use_ns/use_db). Call once at startup when SURREAL_URL is set.
pub async fn init_db() -> Result<(), String> {
    let url = SURREAL_URL.to_string();
    if cfg!(debug_assertions) {
        let try_connect = if url.starts_with("wss") {
            DB.connect::<Wss>(&url).await
        } else {
            DB.connect::<Ws>(&url).await
        };
        println!("Attempting to connect to DB: {:?}", try_connect);
        try_connect.map_err(|e| e.to_string())?;
    } else {
        let result = if url.starts_with("wss") {
            DB.connect::<Wss>(&url).await
        } else {
            DB.connect::<Ws>(&url).await
        };
        match &result {
            Ok(_) => println!("Connected to {}", url),
            Err(e) => println!("Failed connecting to {}: {:?}", url, e),
        }
        result.map_err(|e| e.to_string())?;
    }
    DB.use_ns(NS).use_db(DB_NAME).await.map_err(|e| e.to_string())?;
    println!("Using NS: {}, DB: {}", NS, DB_NAME);
    Ok(())
}

/// Load all piece_costs from the database (call after init_db()).
pub async fn load_piece_costs(db: &Surreal<Client>) -> Result<Vec<PieceCostRow>, String> {
    let rows: Vec<PieceCostRow> = db
        .select("piece_costs")
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Resolved cost and weight for an order item (for display).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemCostWeight {
    pub cost_usd: f64,
    pub weight_g: f64,
}

/// Match an order item to a piece_costs row and return cost/weight for the item's metal type.
pub fn lookup_piece_cost(item: &OrderItem, piece_costs: &[PieceCostRow]) -> Option<ItemCostWeight> {
    let item_name_normalized = item.name.to_lowercase().trim().to_string();
    let item_ring = item.ring_size.as_ref().map(|s| s.trim().to_string());

    // 1) Try match by product_keys
    for row in piece_costs {
        if let Some(keys) = &row.product_keys {
            if keys.iter().any(|k| {
                k.trim().to_lowercase() == item_name_normalized
                    || item.name.to_lowercase().contains(&k.trim().to_lowercase())
            }) {
                if ring_matches(&row.ring_size, &item_ring) {
                    return pick_cost_weight(row, &item.metal_type);
                }
            }
        }
    }

    // 2) Try match by design_key (normalized item name or contains)
    for row in piece_costs {
        let design_lower = row.design_key.to_lowercase();
        if design_lower == item_name_normalized
            || item_name_normalized.contains(&design_lower)
            || design_lower.contains(&item_name_normalized)
        {
            if ring_matches(&row.ring_size, &item_ring) {
                return pick_cost_weight(row, &item.metal_type);
            }
        }
    }

    None
}

fn ring_matches(row_ring: &Option<String>, item_ring: &Option<String>) -> bool {
    match (row_ring, item_ring) {
        (None, _) => true,
        (Some(s), _) if s.is_empty() || s == "N/A" => true,
        (Some(rs), Some(is)) => rs.trim() == is.trim(),
        (Some(_), None) => false,
    }
}

fn pick_cost_weight(row: &PieceCostRow, metal: &MetalType) -> Option<ItemCostWeight> {
    let (cost, weight) = match metal {
        MetalType::Silver => (
            row.silver_usd.unwrap_or(0.0),
            row.silver_g.unwrap_or(0.0),
        ),
        MetalType::Gold => (row.gold_usd.unwrap_or(0.0), row.gold_g.unwrap_or(0.0)),
        MetalType::Bronze => (
            row.bronze_usd.unwrap_or(0.0),
            row.bronze_g.unwrap_or(0.0),
        ),
        MetalType::Unknown => {
            let c = row.silver_usd.unwrap_or(0.0)
                + row.gold_usd.unwrap_or(0.0)
                + row.bronze_usd.unwrap_or(0.0);
            let w = row.silver_g.unwrap_or(0.0)
                + row.gold_g.unwrap_or(0.0)
                + row.bronze_g.unwrap_or(0.0);
            (c, w)
        }
    };
    if cost > 0.0 || weight > 0.0 {
        Some(ItemCostWeight {
            cost_usd: cost,
            weight_g: weight,
        })
    } else {
        None
    }
}
