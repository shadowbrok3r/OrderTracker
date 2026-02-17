//! Etsy API v3 client: OAuth token handling and shop receipts (orders).

use crate::log;
use chrono::{Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::model::{MetalType, Order, OrderItem, OrderSource};

fn etsy_keystring() -> String {
    std::env::var("ETSY_KEYSTRING").unwrap_or_default()
}
fn etsy_secret() -> String {
    std::env::var("ETSY_SECRET").unwrap_or_default()
}
fn etsy_shop_id() -> String {
    std::env::var("ETSY_SHOP_ID").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// OAuth config (refresh token + cached access token)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct EtsyOAuthConfig {
    refresh_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_at_utc_secs: Option<i64>,
}

fn etsy_config_path() -> Option<PathBuf> {
    // HA add-on: persistent storage at /data/
    let ha_path = PathBuf::from("/data/etsy_oauth.json");
    if ha_path.parent().is_some_and(|p| p.exists()) {
        return Some(ha_path);
    }
    // Desktop / local dev: system config directory
    directories::ProjectDirs::from("com", "KingsOfAlchemy", "OrderTracker")
        .map(|d| d.config_dir().join("etsy_oauth.json"))
}

fn load_etsy_config() -> EtsyOAuthConfig {
    let path = match etsy_config_path() {
        Some(p) => p,
        None => return EtsyOAuthConfig::default(),
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return EtsyOAuthConfig::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_etsy_config(cfg: &EtsyOAuthConfig) -> Result<(), String> {
    let path = etsy_config_path().ok_or_else(|| "No config dir".to_string())?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(())
}

async fn get_etsy_access_token() -> Result<String, String> {
    let mut cfg = load_etsy_config();
    let now_secs = Utc::now().timestamp();
    let expires = cfg.expires_at_utc_secs.unwrap_or(0);
    if cfg.access_token.is_some() && expires > now_secs + 300 {
        return Ok(cfg.access_token.as_ref().unwrap().clone());
    }
    if let Some(ref refresh) = cfg.refresh_token {
        let refresh = refresh.clone();
        return refresh_etsy_token_async(&mut cfg, &refresh).await;
    }
    let secret = etsy_secret();
    if !secret.is_empty() {
        return Ok(secret);
    }
    Err("Etsy not connected. Get a refresh token from order-tracker.kingsofalchemy.com/connect and paste it in Settings.".to_string())
}

async fn refresh_etsy_token_async(cfg: &mut EtsyOAuthConfig, refresh_token: &str) -> Result<String, String> {
    let keystring = etsy_keystring();
    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", keystring.as_str()),
        ("refresh_token", refresh_token),
    ];
    let res = reqwest::Client::new()
        .post("https://api.etsy.com/v3/public/oauth/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Refresh request failed: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Etsy token refresh failed: {} - {}", status, body));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        expires_in: Option<u64>,
        refresh_token: Option<String>,
    }
    let tok: TokenResponse = res.json().await.map_err(|e| format!("Parse token response: {}", e))?;
    let expires_in = tok.expires_in.unwrap_or(3600);
    cfg.access_token = Some(tok.access_token.clone());
    cfg.expires_at_utc_secs = Some(Utc::now().timestamp() + expires_in as i64);
    if let Some(rt) = tok.refresh_token {
        cfg.refresh_token = Some(rt);
    }
    let _ = save_etsy_config(cfg);
    Ok(tok.access_token)
}

/// Save a new refresh token (from web OAuth flow). Next API use will refresh the access token.
pub fn save_etsy_refresh_token(refresh_token: String) -> Result<(), String> {
    let mut cfg = load_etsy_config();
    cfg.refresh_token = Some(refresh_token.trim().to_string());
    cfg.access_token = None;
    cfg.expires_at_utc_secs = None;
    save_etsy_config(&cfg)
}

// ---------------------------------------------------------------------------
// Etsy API response types (v3 shop receipts)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct EtsyReceiptsResponse {
    count: Option<i32>,
    results: Vec<EtsyReceipt>,
}

#[derive(Debug, Deserialize)]
struct EtsyReceipt {
    receipt_id: i64,
    #[serde(default)]
    order_id: Option<i64>,
    name: String,
    #[serde(rename = "created_timestamp")]
    create_timestamp: i64,
    #[serde(alias = "total", default)]
    grandtotal: Option<EtsyMoney>,
    transactions: Option<Vec<EtsyTransaction>>,
    first_line: Option<String>,
    formatted_address: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EtsyMoney {
    amount: Option<i64>,
    divisor: Option<i64>,
    currency_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EtsyTransaction {
    title: Option<String>,
    quantity: Option<i32>,
    price: Option<EtsyMoney>,
    variations: Option<Vec<EtsyVariation>>,
    #[serde(default)]
    listing_id: Option<i64>,
    #[serde(default)]
    listing_image_id: Option<i64>,
    #[serde(default)]
    expected_ship_date: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct EtsyVariation {
    formatted_name: Option<String>,
    formatted_value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EtsyListingImage {
    url_75x75: Option<String>,
    url_170x135: Option<String>,
}

async fn fetch_listing_image_urls(
    client: &reqwest::Client,
    access_token: &str,
    x_api_key: &str,
    keys: &[(i64, i64)],
) -> std::collections::HashMap<(i64, i64), String> {
    let mut out = std::collections::HashMap::new();
    for &(listing_id, image_id) in keys {
        let url = format!(
            "https://api.etsy.com/v3/application/listings/{}/images/{}",
            listing_id, image_id
        );
        let resp = client
            .get(&url)
            .header("x-api-key", x_api_key)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await;
        if let Ok(r) = resp {
            if r.status().is_success() {
                if let Ok(img) = r.json::<EtsyListingImage>().await {
                    let u = img
                        .url_170x135
                        .or(img.url_75x75)
                        .filter(|s| !s.is_empty());
                    if let Some(u) = u {
                        out.insert((listing_id, image_id), u);
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

/// Fetch shop receipts (orders) from Etsy API v3 (last 60 days). Only paid, not-yet-shipped.
pub async fn fetch_etsy_orders() -> Result<Vec<Order>, String> {
    log::app_log("INFO", "Etsy: getting access token...");
    let access_token = get_etsy_access_token().await?;
    log::app_log("INFO", "Etsy: token OK, requesting receipts...");
    let client = reqwest::Client::new();
    const LIMIT: i32 = 100;
    let base_url = format!(
        "https://api.etsy.com/v3/application/shops/{}/receipts",
        etsy_shop_id()
    );
    let x_api_key = format!("{}:{}", etsy_keystring(), etsy_secret());

    let mut all_receipts = Vec::new();
    let mut offset = 0i32;

    let was_paid = true;
    let was_shipped = false;
    log::app_log("INFO", format!("Etsy: fetching receipts (was_paid={}, was_shipped={})", was_paid, was_shipped));

    loop {
        let url = format!(
            "{}?limit={}&offset={}&was_paid={}&was_shipped={}",
            base_url, LIMIT, offset, was_paid, was_shipped
        );
        log::app_log("INFO", format!("Etsy: GET receipts offset={}", offset));
        let response = client
            .get(&url)
            .header("x-api-key", &x_api_key)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await
            .map_err(|e| format!("Etsy request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Etsy API error: {} - {}",
                status,
                if body.is_empty() {
                    "Check x-api-key and OAuth token (transactions_r scope)".to_string()
                } else {
                    body
                }
            ));
        }

        let raw_body = response.text().await.map_err(|e| format!("Etsy response read failed: {}", e))?;
        let page: EtsyReceiptsResponse = match serde_json::from_str(&raw_body) {
            Ok(p) => p,
            Err(e) => {
                let preview = if raw_body.len() > 1500 {
                    format!("{}... (truncated)", &raw_body[..1500])
                } else {
                    raw_body.clone()
                };
                log::app_log("ERROR", format!("Etsy parse (offset={}): {}", offset, preview));
                return Err(format!("Etsy response parse failed: {} | raw preview: {}", e, preview));
            }
        };

        let results = page.results;
        let n = results.len() as i32;
        all_receipts.extend(results);
        log::app_log("INFO", format!("Etsy: page offset={} got {} receipts (total so far: {})", offset, n, all_receipts.len()));

        if n < LIMIT {
            break;
        }
        offset += LIMIT;
    }

    log::app_log("INFO", format!("Etsy: {} receipts total, fetching listing images...", all_receipts.len()));

    let mut image_keys: Vec<(i64, i64)> = Vec::new();
    for r in &all_receipts {
        for t in r.transactions.as_deref().unwrap_or(&[]) {
            if let (Some(lid), Some(iid)) = (t.listing_id, t.listing_image_id) {
                image_keys.push((lid, iid));
            }
        }
    }
    image_keys.sort_unstable();
    image_keys.dedup();
    let image_urls: std::collections::HashMap<(i64, i64), String> = fetch_listing_image_urls(
        &client,
        &access_token,
        &x_api_key,
        &image_keys,
    )
    .await;

    log::app_log("INFO", format!("Etsy: got {} image URLs, mapping to orders...", image_urls.len()));

    let two_months_ago = Utc::now() - Duration::days(60);
    let orders: Vec<Order> = all_receipts
        .into_iter()
        .filter_map(|r| {
            let order_ts = r.create_timestamp;
            let order_date = if order_ts > 1_000_000_000_000 {
                Utc.timestamp_millis_opt(order_ts).single().unwrap_or(Utc::now())
            } else {
                Utc.timestamp_opt(order_ts, 0).single().unwrap_or(Utc::now())
            };
            if order_date < two_months_ago {
                return None;
            }
            let due_date = r
                .transactions
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter_map(|t| t.expected_ship_date)
                .max()
                .and_then(|ts| {
                    if ts > 1_000_000_000_000 {
                        Utc.timestamp_millis_opt(ts).single()
                    } else {
                        Utc.timestamp_opt(ts, 0).single()
                    }
                })
                .unwrap_or_else(|| order_date + Duration::days(14));

            let (total_price, currency) = if let Some(ref total_money) = r.grandtotal {
                let divisor = total_money.divisor.unwrap_or(100).max(1) as f64;
                let price = (total_money.amount.unwrap_or(0) as f64) / divisor;
                let curr = total_money
                    .currency_code
                    .clone()
                    .unwrap_or_else(|| "USD".to_string());
                (price, curr)
            } else {
                (0.0, "USD".to_string())
            };

            let items: Vec<OrderItem> = r
                .transactions
                .unwrap_or_default()
                .into_iter()
                .map(|t| {
                    let title = t.title.unwrap_or_else(|| "Item".to_string());
                    let qty = t.quantity.unwrap_or(1);
                    let price_val = t
                        .price
                        .as_ref()
                        .map(|p| {
                            let div = p.divisor.unwrap_or(100).max(1) as f64;
                            (p.amount.unwrap_or(0) as f64) / div
                        })
                        .unwrap_or(0.0);
                    let variant_parts: Vec<String> = t
                        .variations
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|v| {
                            let n = v.formatted_name.unwrap_or_default();
                            let val = v.formatted_value.unwrap_or_default();
                            if n.is_empty() && val.is_empty() {
                                None
                            } else {
                                Some(format!("{}: {}", n, val))
                            }
                        })
                        .collect();
                    let variant_info = if variant_parts.is_empty() {
                        None
                    } else {
                        Some(variant_parts.join(", "))
                    };
                    let full_name = format!("{} {}", &title, variant_info.as_deref().unwrap_or(""));
                    let metal_type = MetalType::from_string(&full_name);
                    let ring_size = variant_parts
                        .iter()
                        .find(|s| {
                            s.to_lowercase().contains("ring") || s.to_lowercase().contains("size")
                        })
                        .cloned();

                    let image_url = t
                        .listing_id
                        .zip(t.listing_image_id)
                        .and_then(|k| image_urls.get(&k).cloned());
                    OrderItem {
                        name: title,
                        quantity: qty as u32,
                        price: price_val,
                        metal_type,
                        ring_size,
                        variant_info,
                        image_url,
                    }
                })
                .collect();

            let total_price = if total_price > 0.0 {
                total_price
            } else {
                items.iter().map(|i| i.price * i.quantity as f64).sum::<f64>()
            };

            let shipping_address = r.first_line.clone().or(r.formatted_address.clone());

            Some(Order {
                id: r.receipt_id.to_string(),
                source: OrderSource::Etsy,
                order_number: format!("#{}", r.order_id.unwrap_or(r.receipt_id)),
                customer_name: {
                    let n = r.name.trim().to_string();
                    if n.is_empty() {
                        "Unknown".to_string()
                    } else {
                        n
                    }
                },
                items,
                order_date,
                due_date,
                total_price,
                currency,
                status: r.status.unwrap_or_else(|| "open".to_string()),
                shipping_address,
            })
        })
        .collect();

    log::app_log("INFO", format!("Etsy: built {} orders", orders.len()));
    Ok(orders)
}
