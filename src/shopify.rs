//! Shopify API client: fetch orders and map to shared [crate::model] types.

use crate::log;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::model::{MetalType, Order, OrderItem, OrderSource};

fn shopify_url() -> String {
    std::env::var("SHOPIFY_URL").unwrap_or_default()
}
fn shopify_access_token() -> String {
    std::env::var("SHOPIFY_ACCESS_TOKEN").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Shopify API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ShopifyOrdersResponse {
    orders: Vec<ShopifyOrder>,
}

#[derive(Debug, Deserialize)]
struct ShopifyOrder {
    id: i64,
    order_number: i64,
    created_at: String,
    customer: Option<ShopifyCustomer>,
    line_items: Vec<ShopifyLineItem>,
    total_price: String,
    currency: String,
    fulfillment_status: Option<String>,
    shipping_address: Option<ShopifyAddress>,
}

#[derive(Debug, Deserialize)]
struct ShopifyCustomer {
    first_name: Option<String>,
    last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ShopifyLineItem {
    name: String,
    quantity: i32,
    price: String,
    variant_title: Option<String>,
    properties: Option<Vec<ShopifyProperty>>,
}

#[derive(Debug, Deserialize)]
struct ShopifyProperty {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct ShopifyAddress {
    address1: Option<String>,
    city: Option<String>,
    province: Option<String>,
    country: Option<String>,
    zip: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_ring_size(name: &str, properties: &Option<Vec<ShopifyProperty>>) -> Option<String> {
    if let Some(props) = properties {
        for prop in props {
            let prop_name_lower = prop.name.to_lowercase();
            if prop_name_lower.contains("size") || prop_name_lower.contains("ring") {
                return Some(prop.value.clone());
            }
        }
    }
    let lower = name.to_lowercase();
    let patterns = ["size ", "ring size ", "sz ", "us ", "uk "];
    for pattern in patterns {
        if let Some(idx) = lower.find(pattern) {
            let start = idx + pattern.len();
            let remaining = &name[start..];
            let size: String = remaining
                .chars()
                .take_while(|c| c.is_numeric() || *c == '.' || *c == '/' || *c == ' ')
                .collect();
            if !size.trim().is_empty() {
                return Some(size.trim().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Fetch orders from Shopify (last 60 days, any status).
pub async fn fetch_shopify_orders() -> Result<Vec<Order>, String> {
    log::app_log("INFO", "Shopify: requesting orders (last 60 days)...");
    let client = reqwest::Client::new();
    let two_months_ago = Utc::now() - Duration::days(60);
    let created_at_min = two_months_ago.format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    let url = format!(
        "{}/orders.json?status=any&limit=250&created_at_min={}",
        shopify_url(),
        created_at_min
    );

    let response = client
        .get(&url)
        .header("X-Shopify-Access-Token", shopify_access_token())
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("Shopify request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Shopify API error: {}", response.status()));
    }

    let shopify_response: ShopifyOrdersResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Shopify response: {}", e))?;

    log::app_log("INFO", format!("Shopify: got {} orders, mapping...", shopify_response.orders.len()));

    let orders = shopify_response
        .orders
        .into_iter()
        .map(|so| {
            let order_date = DateTime::parse_from_rfc3339(&so.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let due_date = order_date + Duration::days(14);
            let customer_name = so
                .customer
                .map(|c| {
                    format!(
                        "{} {}",
                        c.first_name.unwrap_or_default(),
                        c.last_name.unwrap_or_default()
                    )
                    .trim()
                    .to_string()
                })
                .unwrap_or_else(|| "Unknown Customer".to_string());

            let items: Vec<OrderItem> = so
                .line_items
                .into_iter()
                .map(|li| {
                    let full_name = format!(
                        "{} {}",
                        li.name,
                        li.variant_title.clone().unwrap_or_default()
                    );
                    let metal_type = MetalType::from_string(&full_name);
                    let ring_size = extract_ring_size(&full_name, &li.properties);
                    OrderItem {
                        name: li.name,
                        quantity: li.quantity as u32,
                        price: li.price.parse().unwrap_or(0.0),
                        metal_type,
                        ring_size,
                        variant_info: li.variant_title,
                        image_url: None,
                    }
                })
                .collect();

            let shipping_address = so.shipping_address.map(|addr| {
                format!(
                    "{}, {}, {} {} {}",
                    addr.address1.unwrap_or_default(),
                    addr.city.unwrap_or_default(),
                    addr.province.unwrap_or_default(),
                    addr.zip.unwrap_or_default(),
                    addr.country.unwrap_or_default()
                )
            });

            Order {
                id: so.id.to_string(),
                source: OrderSource::Shopify,
                order_number: format!("#{}", so.order_number),
                customer_name,
                items,
                order_date,
                due_date,
                total_price: so.total_price.parse().unwrap_or(0.0),
                currency: so.currency,
                status: so.fulfillment_status.unwrap_or_else(|| "unfulfilled".to_string()),
                shipping_address,
            }
        })
        .collect();

    Ok(orders)
}
