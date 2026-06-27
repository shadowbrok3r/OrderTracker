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

fn tokenize(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || matches!(c, ',' | '|' | ':'))
        .map(|w| w.trim())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect()
}

fn is_length_unit(s: &str) -> bool {
    s.starts_with("in") || s.starts_with('"') || s.starts_with("cm") || s.starts_with("mm")
}

/// Extract a US ring size and format it as "{n} US". Tolerates surrounding words
/// ("US 7", "Size 7", "Silver / 9"), half sizes ("7 1/2" -> "7.5 US"), and
/// ignores karats ("14k") and chain lengths ("18 inch"). Only 3.0..=16.0 counts.
pub fn format_ring_size(raw: &str) -> Option<String> {
    fn fmt(v: f64) -> Option<String> {
        if !(3.0..=16.0).contains(&v) {
            return None;
        }
        let n = if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{}", v) };
        Some(format!("{} US", n))
    }
    let words = tokenize(raw);
    for (i, w) in words.iter().enumerate() {
        let wl = w.to_lowercase();
        if wl.ends_with('k') || wl.ends_with("kt") {
            continue; // karat, not a size
        }
        let num: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if num.is_empty() {
            continue;
        }
        let Ok(mut v) = num.trim_matches('.').parse::<f64>() else { continue };
        let rest = w.chars().skip_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>().to_lowercase();
        if is_length_unit(&rest) {
            continue; // e.g. 18in, 9"
        }
        if let Some(next) = words.get(i + 1) {
            match next.as_str() {
                "1/2" => v += 0.5,
                "1/4" => v += 0.25,
                "3/4" => v += 0.75,
                _ => {}
            }
            if is_length_unit(&next.to_lowercase()) {
                continue; // a length, not a ring size
            }
        }
        if let Some(s) = fmt(v) {
            return Some(s);
        }
    }
    // no-space slash form, e.g. "Silver/9"
    raw.split('/').find_map(|p| p.trim().parse::<f64>().ok().and_then(fmt))
}

/// Extract a chain/necklace length and format it as "{n} in" / "{n} cm" /
/// "{n} mm". Requires an explicit length unit, so plain numbers and karats
/// never count.
pub fn format_length(raw: &str) -> Option<String> {
    let words = tokenize(raw);
    for (i, w) in words.iter().enumerate() {
        let num: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if num.is_empty() {
            continue;
        }
        let Ok(v) = num.trim_matches('.').parse::<f64>() else { continue };
        let rest = w.chars().skip_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>().to_lowercase();
        let next = words.get(i + 1).map(|s| s.to_lowercase()).unwrap_or_default();
        let unit = if rest.starts_with("in") || rest.starts_with('"') || next.starts_with("in") || next.starts_with('"') {
            Some("in")
        } else if rest.starts_with("cm") || next.starts_with("cm") {
            Some("cm")
        } else if rest.starts_with("mm") || next.starts_with("mm") {
            Some("mm")
        } else {
            None
        };
        if let Some(u) = unit {
            let n = if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{}", v) };
            return Some(format!("{} {}", n, u));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{format_length, format_ring_size};

    #[test]
    fn ring_sizes() {
        assert_eq!(format_ring_size("Ring Size: US 7").as_deref(), Some("7 US"));
        assert_eq!(format_ring_size("Size: 7").as_deref(), Some("7 US"));
        assert_eq!(format_ring_size("Silver / 9").as_deref(), Some("9 US"));
        assert_eq!(format_ring_size("Silver/9").as_deref(), Some("9 US"));
        assert_eq!(format_ring_size("13 / .925 Silver").as_deref(), Some("13 US"));
        assert_eq!(format_ring_size("Ring Size: 7 1/2").as_deref(), Some("7.5 US"));
        assert_eq!(format_ring_size("7.5").as_deref(), Some("7.5 US"));
    }

    #[test]
    fn not_ring_sizes() {
        assert_eq!(format_ring_size("14k Gold"), None);
        assert_eq!(format_ring_size("18 inch"), None);
        assert_eq!(format_ring_size("Metal: Silver"), None);
        assert_eq!(format_ring_size("14k Gold Plated / 9").as_deref(), Some("9 US"));
    }

    #[test]
    fn lengths() {
        assert_eq!(format_length("Length: 18 inches").as_deref(), Some("18 in"));
        assert_eq!(format_length("18\"").as_deref(), Some("18 in"));
        assert_eq!(format_length("20in").as_deref(), Some("20 in"));
        assert_eq!(format_length("45 cm").as_deref(), Some("45 cm"));
        assert_eq!(format_length("Silver"), None);
        assert_eq!(format_length("Ring Size: 9"), None);
    }
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
