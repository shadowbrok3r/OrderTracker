//! Shared domain types for orders (used by UI and by Etsy/Shopify API modules).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[cfg(feature = "server")]
use surrealdb_types::SurrealValue;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetalType {
    Gold,
    Silver,
    Bronze,
    Unknown,
}

impl MetalType {
    /// Parse metal type from product name/variant text.
    pub fn from_string(s: &str) -> Self {
        let lower = s.to_lowercase();
        if lower.contains("gold") || lower.contains("14k") || lower.contains("18k") || lower.contains("10k") {
            MetalType::Gold
        } else if lower.contains("silver") || lower.contains("sterling") || lower.contains("925") {
            MetalType::Silver
        } else if lower.contains("bronze") || lower.contains("brass") {
            MetalType::Bronze
        } else {
            MetalType::Unknown
        }
    }

    pub fn display_class(&self) -> &'static str {
        match self {
            MetalType::Gold => "badge-gold",
            MetalType::Silver => "badge-silver",
            MetalType::Bronze => "badge-bronze",
            MetalType::Unknown => "badge-nebula",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            MetalType::Gold => "Gold Plated",
            MetalType::Silver => "Silver",
            MetalType::Bronze => "Bronze",
            MetalType::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OrderSource {
    Shopify,
    Etsy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub source: OrderSource,
    pub order_number: String,
    pub customer_name: String,
    pub items: Vec<OrderItem>,
    pub order_date: DateTime<Utc>,
    pub due_date: DateTime<Utc>,
    pub total_price: f64,
    pub currency: String,
    pub status: String,
    pub shipping_address: Option<String>,
}

impl Order {
    pub fn days_until_due(&self) -> i64 {
        let now = Utc::now();
        (self.due_date - now).num_days()
    }

    pub fn urgency_class(&self) -> &'static str {
        let days = self.days_until_due();
        if days < 0 {
            "urgency-overdue"
        } else if days <= 3 {
            "urgency-critical"
        } else if days <= 7 {
            "urgency-warning"
        } else {
            "urgency-ok"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderItem {
    pub name: String,
    pub quantity: u32,
    pub price: f64,
    pub metal_type: MetalType,
    pub ring_size: Option<String>,
    pub variant_info: Option<String>,
    /// Product thumbnail URL (from Etsy listing image or Shopify line item image).
    pub image_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Piece cost types & matching (shared between server DB logic and client UI)
// ---------------------------------------------------------------------------

/// One row from piece_costs table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "server", derive(SurrealValue))]
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

/// Resolved cost and weight for an order item (for display).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
