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
    #[serde(default)]
    product_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ShopifyProductResponse {
    product: ShopifyProduct,
}

#[derive(Debug, Deserialize)]
struct ShopifyProduct {
    image: Option<ShopifyImage>,
}

#[derive(Debug, Deserialize)]
struct ShopifyImage {
    src: Option<String>,
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

fn extract_size(
    variant_title: &Option<String>,
    name: &str,
    properties: &Option<Vec<ShopifyProperty>>,
) -> Option<String> {
    // 1) explicit size/ring line-item property
    if let Some(props) = properties {
        for prop in props {
            let n = prop.name.to_lowercase();
            if n.contains("size") || n.contains("ring") {
                if let Some(s) = crate::model::format_ring_size(&prop.value) {
                    return Some(s);
                }
            }
        }
    }
    // 2) the variant title (e.g. "9", "Silver / 9", "Size 9")
    if let Some(vt) = variant_title {
        if let Some(s) = crate::model::format_ring_size(vt) {
            return Some(s);
        }
    }
    // 3) a "size N" token in the product name
    let lower = name.to_lowercase();
    for pat in ["ring size ", "size ", "sz "] {
        if let Some(idx) = lower.find(pat) {
            if let Some(s) = crate::model::format_ring_size(&name[idx + pat.len()..]) {
                return Some(s);
            }
        }
    }
    // 4) necklace/chain length fallback
    if let Some(props) = properties {
        for prop in props {
            let n = prop.name.to_lowercase();
            if n.contains("length") || n.contains("inch") || n.contains("size") || n.contains("necklace") || n.contains("chain") {
                if let Some(s) = crate::model::format_length(&prop.value) {
                    return Some(s);
                }
            }
        }
    }
    if let Some(vt) = variant_title {
        if let Some(s) = crate::model::format_length(vt) {
            return Some(s);
        }
    }
    crate::model::format_length(name)
}

/// Fetch the primary image URL for each product id.
async fn fetch_product_image_urls(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    ids: &[i64],
) -> std::collections::HashMap<i64, String> {
    let mut out = std::collections::HashMap::new();
    for &id in ids {
        let url = format!("{}/products/{}.json?fields=id,image", base_url, id);
        let resp = client
            .get(&url)
            .header("X-Shopify-Access-Token", token)
            .send()
            .await;
        if let Ok(r) = resp {
            if r.status().is_success() {
                if let Ok(p) = r.json::<ShopifyProductResponse>().await {
                    let src = p.product.image.and_then(|i| i.src).filter(|s| !s.is_empty());
                    if let Some(src) = src {
                        out.insert(id, src);
                    }
                }
            }
        }
    }
    out
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

    let mut product_ids: Vec<i64> = shopify_response
        .orders
        .iter()
        .flat_map(|o| o.line_items.iter().filter_map(|li| li.product_id))
        .collect();
    product_ids.sort_unstable();
    product_ids.dedup();
    log::app_log("INFO", format!("Shopify: fetching images for {} products...", product_ids.len()));
    let image_urls =
        fetch_product_image_urls(&client, &shopify_url(), &shopify_access_token(), &product_ids).await;

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
                    let ring_size = extract_size(&li.variant_title, &li.name, &li.properties);
                    let image_url = li.product_id.and_then(|id| image_urls.get(&id).cloned());
                    OrderItem {
                        name: li.name,
                        quantity: li.quantity as u32,
                        price: li.price.parse().unwrap_or(0.0),
                        metal_type,
                        ring_size,
                        variant_info: li.variant_title,
                        image_url,
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
                archived: false,
                completed: false,
            }
        })
        .collect();

    Ok(orders)
}
