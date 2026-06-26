//! Types shared between OrderTracker and jewelry_cost_calculator.

use serde::{Deserialize, Serialize};

#[cfg(feature = "surreal")]
use surrealdb_types::SurrealValue;

/// Table the calculator writes and OrderTracker reads.
pub const PIECE_COSTS_TABLE: &str = "piece_costs";

/// One row of the piece_costs catalog: per-(design, size) weight + cost by metal.
/// `gold_*` holds the single karat the shop casts (14K).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "surreal", derive(SurrealValue))]
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
