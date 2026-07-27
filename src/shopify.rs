//! Shopify API client: fetch orders and map to shared [crate::model] types.

use crate::log;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Deserialize;

use crate::model::{MetalType, Order, OrderItem, OrderSource};

fn shopify_url() -> String {
    std::env::var("SHOPIFY_URL").unwrap_or_default()
}
fn shopify_access_token() -> String {
    std::env::var("SHOPIFY_ACCESS_TOKEN").unwrap_or_default()
}

/// Rolling window for a routine refresh; open orders are pulled regardless of age.
pub fn lookback_days() -> i64 {
    std::env::var("ORDER_LOOKBACK_DAYS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|d| *d > 0)
        .unwrap_or(90)
}

/// Page cap for one orders.json sweep (250 orders/page).
const MAX_ORDER_PAGES: usize = 200;
/// Consecutive 429 retries tolerated per sweep before giving up.
const MAX_RATE_LIMIT_RETRIES: u32 = 8;
/// Cap on per-product image lookups in one fetch.
const MAX_IMAGE_LOOKUPS: usize = 400;

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
    #[serde(default)]
    cancelled_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
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
    if ids.len() > MAX_IMAGE_LOOKUPS {
        log::app_log(
            "ERROR",
            format!(
                "Shopify: {} products need images but only the first {} are looked up; \
                 the rest have no thumbnail.",
                ids.len(),
                MAX_IMAGE_LOOKUPS
            ),
        );
    }
    for &id in ids.iter().take(MAX_IMAGE_LOOKUPS) {
        let url = format!("{}/products/{}.json?fields=id,image", base_url, id);
        let mut retries = 0u32;
        loop {
            let resp = client
                .get(&url)
                .header("X-Shopify-Access-Token", token)
                .send()
                .await;
            let r = match resp {
                Ok(r) => r,
                Err(_) => break,
            };
            if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if retries >= MAX_RATE_LIMIT_RETRIES {
                    log::app_log(
                        "ERROR",
                        format!(
                            "Shopify: rate limited on the image for product {} after {} retries; skipped.",
                            id, MAX_RATE_LIMIT_RETRIES
                        ),
                    );
                    break;
                }
                retries += 1;
                let wait = r
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<f32>().ok())
                    .unwrap_or(2.0);
                tokio::time::sleep(std::time::Duration::from_secs_f32(wait)).await;
                continue;
            }
            if r.status().is_success() {
                if let Ok(p) = r.json::<ShopifyProductResponse>().await {
                    let src = p.product.image.and_then(|i| i.src).filter(|s| !s.is_empty());
                    if let Some(src) = src {
                        out.insert(id, src);
                    }
                }
            }
            break;
        }
    }
    out
}

/// Paginate orders.json for one status filter, following the Link header cursor.
/// `created_at_min` applies to the first request only — Shopify forbids any
/// parameter other than `limit` and `fields` alongside `page_info`.
async fn sweep_orders(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    order_status: &str,
    created_at_min: Option<DateTime<Utc>>,
) -> Result<Vec<ShopifyOrder>, String> {
    let mut url = match created_at_min {
        Some(t) => format!(
            "{}/orders.json?status={}&limit=250&created_at_min={}",
            base,
            order_status,
            t.to_rfc3339_opts(SecondsFormat::Secs, true)
        ),
        None => format!("{}/orders.json?status={}&limit=250", base, order_status),
    };
    let mut out: Vec<ShopifyOrder> = Vec::new();
    let mut page = 0usize;
    let mut retries = 0u32;
    loop {
        let resp = client
            .get(&url)
            .header("X-Shopify-Access-Token", token)
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| format!("Shopify request failed: {}", e))?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            if retries >= MAX_RATE_LIMIT_RETRIES {
                return Err(format!(
                    "Shopify rate limited on status={} after {} retries ({} orders collected)",
                    order_status,
                    MAX_RATE_LIMIT_RETRIES,
                    out.len()
                ));
            }
            retries += 1;
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(2.0);
            tokio::time::sleep(std::time::Duration::from_secs_f32(wait)).await;
            continue;
        }
        if !resp.status().is_success() {
            return Err(format!("Shopify API error: {}", resp.status()));
        }
        retries = 0;
        let link = resp
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body: ShopifyOrdersResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse Shopify response: {}", e))?;
        page += 1;
        let n = body.orders.len();
        out.extend(body.orders);
        log::app_log(
            "INFO",
            format!(
                "Shopify: status={} page {} got {} orders (total {})",
                order_status,
                page,
                n,
                out.len()
            ),
        );
        match link.as_deref().and_then(next_page_info) {
            Some(pi) => {
                if page >= MAX_ORDER_PAGES {
                    log::app_log(
                        "ERROR",
                        format!(
                            "Shopify: stopped at the {}-page cap for status={} with {} orders; \
                             more pages remain and were NOT fetched.",
                            MAX_ORDER_PAGES,
                            order_status,
                            out.len()
                        ),
                    );
                    break;
                }
                // page_info requests may carry only limit and fields.
                url = format!("{}/orders.json?limit=250&page_info={}", base, pi);
            }
            None => break,
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Fetch orders from Shopify, paginating every page via the Link header.
/// `Some(t)` is the routine refresh: every OPEN order regardless of age, plus all
/// orders created since `t`. `None` is a full history backfill.
pub async fn fetch_shopify_orders(
    created_at_min: Option<DateTime<Utc>>,
) -> Result<Vec<Order>, String> {
    let base = shopify_url();
    let token = shopify_access_token();
    if base.is_empty() || token.is_empty() {
        return Err("Shopify not configured".to_string());
    }
    let client = reqwest::Client::new();

    let mut raw: Vec<ShopifyOrder> = Vec::new();
    match created_at_min {
        Some(t) => {
            log::app_log(
                "INFO",
                format!(
                    "Shopify: open orders (all ages) + everything since {}...",
                    t.format("%Y-%m-%d")
                ),
            );
            // No date bound on open orders so an old unfulfilled order is never missed.
            raw.extend(sweep_orders(&client, &base, &token, "open", None).await?);
            raw.extend(sweep_orders(&client, &base, &token, "any", Some(t)).await?);
        }
        None => {
            log::app_log("INFO", "Shopify: full history backfill (all orders, all time)...");
            raw.extend(sweep_orders(&client, &base, &token, "any", None).await?);
        }
    }

    let before = raw.len();
    let mut seen = std::collections::HashSet::new();
    raw.retain(|o| seen.insert(o.id));
    log::app_log(
        "INFO",
        format!(
            "Shopify: {} orders after dedupe ({} raw), mapping...",
            raw.len(),
            before
        ),
    );

    if created_at_min.is_none() {
        if let Some(oldest) = raw
            .iter()
            .filter_map(|o| DateTime::parse_from_rfc3339(&o.created_at).ok())
            .min()
        {
            let age = (Utc::now() - oldest.with_timezone(&Utc)).num_days();
            log::app_log(
                "INFO",
                format!(
                    "Shopify: oldest order returned is {} days old ({}).",
                    age,
                    oldest.format("%Y-%m-%d")
                ),
            );
            if age < 70 {
                log::app_log(
                    "ERROR",
                    "Shopify: backfill reached only ~60 days. The Order resource is capped at \
                     60 days without the read_all_orders scope, so older orders were not returned.",
                );
            }
        }
    }

    let shopify_response = ShopifyOrdersResponse { orders: raw };

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
                    // Metal from a material/metal property first, then the
                    // variant title, then any property, then the product name
                    // last (so a "Sterling Silver ..." title can't override a
                    // Bronze material selection).
                    let metal_type = li
                        .properties
                        .as_ref()
                        .and_then(|props| {
                            props
                                .iter()
                                .filter(|p| {
                                    let n = p.name.to_lowercase();
                                    n.contains("material") || n.contains("metal")
                                })
                                .find_map(|p| MetalType::from_string_opt(&p.value))
                        })
                        .or_else(|| li.variant_title.as_deref().and_then(MetalType::from_string_opt))
                        .or_else(|| {
                            li.properties
                                .as_ref()
                                .and_then(|props| props.iter().find_map(|p| MetalType::from_string_opt(&p.value)))
                        })
                        .or_else(|| MetalType::from_string_opt(&li.name))
                        .unwrap_or(MetalType::Unknown);
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

            // Seed only; the DB overlay replaces it downstream.
            let completed = so.cancelled_at.is_some()
                || so.closed_at.is_some()
                || so.fulfillment_status.as_deref() == Some("fulfilled");

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
                status: if so.cancelled_at.is_some() {
                    "cancelled".to_string()
                } else if so.closed_at.is_some() {
                    so.fulfillment_status.unwrap_or_else(|| "closed".to_string())
                } else {
                    so.fulfillment_status.unwrap_or_else(|| "unfulfilled".to_string())
                },
                shipping_address,
                archived: false,
                completed,
                notes: None,
                stage: None,
            }
        })
        .collect();

    Ok(orders)
}

// ---------------------------------------------------------------------------
// Listings (products) — bulk pull for catalog linking
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ShopifyListingsResponse {
    products: Vec<ShopifyListing>,
}

#[derive(Debug, Deserialize)]
struct ShopifyListing {
    id: i64,
    title: String,
    image: Option<ShopifyImage>,
    #[serde(default)]
    variants: Vec<ShopifyListingVariant>,
}

#[derive(Debug, Deserialize)]
struct ShopifyListingVariant {
    #[serde(default)]
    price: Option<String>,
}

/// Extract the page_info cursor from the rel="next" segment of a Link header.
fn next_page_info(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        if part.contains("rel=\"next\"") {
            let start = part.find('<')? + 1;
            let end = part.find('>')?;
            for kv in part[start..end].split(['?', '&']) {
                if let Some(pi) = kv.strip_prefix("page_info=") {
                    return Some(pi.to_string());
                }
            }
        }
    }
    None
}

/// Fetch ALL Shopify products (cursor pagination via the Link header). Price is
/// the minimum variant price; image is the product's primary image.
pub async fn fetch_shopify_listings() -> Result<Vec<crate::model::Listing>, String> {
    let base = shopify_url();
    let token = shopify_access_token();
    if base.is_empty() || token.is_empty() {
        return Err("Shopify not configured".to_string());
    }
    let client = reqwest::Client::new();
    let fields = "id,title,handle,status,image,variants";
    let mut url = format!("{}/products.json?limit=250&fields={}", base, fields);
    let mut out = Vec::new();
    loop {
        let resp = client
            .get(&url)
            .header("X-Shopify-Access-Token", &token)
            .send()
            .await
            .map_err(|e| format!("Shopify request failed: {}", e))?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(2.0);
            tokio::time::sleep(std::time::Duration::from_secs_f32(wait)).await;
            continue;
        }
        if !resp.status().is_success() {
            return Err(format!("Shopify API error: {}", resp.status()));
        }
        let link = resp
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body: ShopifyListingsResponse = resp
            .json()
            .await
            .map_err(|e| format!("Shopify listings parse failed: {}", e))?;
        for p in body.products {
            let price = p
                .variants
                .iter()
                .filter_map(|v| v.price.as_ref().and_then(|s| s.parse::<f64>().ok()))
                .fold(f64::INFINITY, f64::min);
            out.push(crate::model::Listing {
                source: "shopify".to_string(),
                id: p.id.to_string(),
                title: p.title,
                price: if price.is_finite() { price } else { 0.0 },
                image_url: p.image.and_then(|i| i.src),
            });
        }
        match link.as_deref().and_then(next_page_info) {
            Some(pi) => url = format!("{}/products.json?limit=250&fields={}&page_info={}", base, fields, pi),
            None => break,
        }
    }
    Ok(out)
}
