//! Shared domain types for orders (used by UI and by Etsy/Shopify API modules).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use jewelry_shared::{CatalogPiece, PieceCostRow, PieceCostSize};

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
    Custom,
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
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub completed: bool,
}

impl Order {
    /// Stable key for the persistent order_state overlay (e.g. "shopify_123").
    pub fn state_key(&self) -> String {
        let src = match self.source {
            OrderSource::Shopify => "shopify",
            OrderSource::Etsy => "etsy",
            OrderSource::Custom => "custom",
        };
        format!("{}_{}", src, self.id)
    }

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

/// Resolved cost and weight for an order item (for display).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemCostWeight {
    pub cost_usd: f64,
    pub weight_g: f64,
}

/// Match an order item to a catalog piece + size, returning cost/weight for the item's metal.
pub fn lookup_piece_cost(item: &OrderItem, catalog: &[CatalogPiece]) -> Option<ItemCostWeight> {
    let item_name = item.name.to_lowercase();
    let item_compact: String = item_name.chars().filter(|c| c.is_alphanumeric()).collect();
    let item_ring = item.ring_size.as_ref().map(|s| s.trim().to_string());

    let piece = catalog.iter().find(|p| {
        if let Some(keys) = &p.product_keys {
            if keys.iter().any(|k| {
                let kl = k.trim().to_lowercase();
                !kl.is_empty() && (kl == item_name || item_name.contains(&kl))
            }) {
                return true;
            }
        }
        let name_compact: String =
            p.name.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        !name_compact.is_empty()
            && (item_compact.contains(&name_compact) || name_compact.contains(&item_compact))
    })?;

    let size = piece.sizes.iter().find(|s| ring_matches(&s.ring_size, &item_ring))?;
    pick_cost_weight_size(size, &item.metal_type)
}

fn ring_matches(row_ring: &Option<String>, item_ring: &Option<String>) -> bool {
    match (row_ring, item_ring) {
        (None, _) => true,
        (Some(s), _) if is_wildcard_size(s) => true,
        (Some(rs), Some(is)) => ring_size_key(rs) == ring_size_key(is),
        (Some(_), None) => false,
    }
}

fn is_wildcard_size(s: &str) -> bool {
    let t = s.trim();
    t.is_empty() || t.eq_ignore_ascii_case("N/A")
}

// Numeric core of a ring size so "US 9" matches "9" and "US 8.75" matches "8.75".
fn ring_size_key(s: &str) -> String {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    match digits.trim_matches('.').parse::<f64>() {
        Ok(v) => format!("{}", (v * 100.0).round() / 100.0),
        Err(_) => s.trim().to_lowercase(),
    }
}

/// Extract a US ring size from a variant/property string and format it as
/// "{n} US" (e.g. "Ring Size: 9" -> "9 US", "Silver / 9" -> "9 US"). Only a
/// whole token that parses to 3.0..=16.0 counts, so metal karats ("14k") and
/// chain lengths ("9 inch") are ignored.
pub fn format_ring_size(raw: &str) -> Option<String> {
    fn as_us(t: &str) -> Option<String> {
        let v: f64 = t.trim().parse().ok()?;
        if !(3.0..=16.0).contains(&v) {
            return None;
        }
        let n = if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{}", v) };
        Some(format!("{} US", n))
    }
    if let Some(s) = as_us(raw) {
        return Some(s);
    }
    raw.split(|c| c == '/' || c == ',' || c == '|' || c == ':')
        .find_map(as_us)
}

fn pick_cost_weight_size(s: &PieceCostSize, metal: &MetalType) -> Option<ItemCostWeight> {
    let (cost, weight) = match metal {
        MetalType::Silver => (s.silver_usd.unwrap_or(0.0), s.silver_g.unwrap_or(0.0)),
        // "Gold Plated" pieces are cast in silver then plated, so cost from the silver base.
        MetalType::Gold => (s.silver_usd.unwrap_or(0.0), s.silver_g.unwrap_or(0.0)),
        MetalType::Bronze => (s.bronze_usd.unwrap_or(0.0), s.bronze_g.unwrap_or(0.0)),
        MetalType::Unknown => (s.silver_usd.unwrap_or(0.0), s.silver_g.unwrap_or(0.0)),
    };
    if cost > 0.0 || weight > 0.0 {
        Some(ItemCostWeight { cost_usd: cost, weight_g: weight })
    } else {
        None
    }
}
