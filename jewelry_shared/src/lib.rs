//! Types shared between OrderTracker and jewelry_cost_calculator.

use serde::{Deserialize, Serialize};

#[cfg(feature = "surreal")]
use surrealdb_types::SurrealValue;

/// Table the calculator writes and OrderTracker reads.
pub const PIECE_COSTS_TABLE: &str = "piece_costs";
/// Normalized catalog of distinct pieces; piece_costs link here.
pub const JEWELRY_TABLE: &str = "jewelry";

/// One row of the piece_costs catalog: per-(design, size) weight + cost by metal.
/// `gold_*` holds the single karat the shop casts (14K). `design_key` carries the
/// jewelry slug; the calculator maps it to a `record<jewelry>` link on write.
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

/// A distinct piece of jewelry (normalized). `piece_costs.design_key` links here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "surreal", derive(SurrealValue))]
pub struct Jewelry {
    pub name: String,
    pub kind: String,
    pub product_keys: Option<Vec<String>>,
}

/// Per-size cost row without the design link, nested under a [CatalogPiece].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "surreal", derive(SurrealValue))]
pub struct PieceCostSize {
    pub ring_size: Option<String>,
    pub volume_cm3: Option<f64>,
    pub silver_g: Option<f64>,
    pub silver_usd: Option<f64>,
    pub gold_g: Option<f64>,
    pub gold_usd: Option<f64>,
    pub bronze_g: Option<f64>,
    pub bronze_usd: Option<f64>,
    pub wax_usd: Option<f64>,
}

/// A jewelry piece with all its sizes — the OrderTracker catalog view unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "surreal", derive(SurrealValue))]
pub struct CatalogPiece {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub product_keys: Option<Vec<String>>,
    #[serde(default)]
    pub sizes: Vec<PieceCostSize>,
}
