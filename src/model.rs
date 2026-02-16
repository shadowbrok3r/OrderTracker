//! Shared domain types for orders (used by UI and by Etsy/Shopify API modules).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
