#![allow(non_snake_case)]

use chrono::{DateTime, Duration, TimeZone, Utc};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Environment variables for API tokens
pub const ETSY_KEYSTRING: &str = env!("ETSY_KEYSTRING");
/// Etsy app shared secret (for x-api-key header). In .env as ETSY_SECRET.
pub const ETSY_SECRET: &str = env!("ETSY_SECRET");
pub const ETSY_SHOP_ID: &str = env!("ETSY_SHOP_ID");
pub const SHOPIFY_URL: &str = env!("SHOPIFY_URL");
pub const SHOPIFY_ACCESS_TOKEN: &str = env!("SHOPIFY_ACCESS_TOKEN");

// ============================================================================
// Etsy OAuth (refresh token stored in config; access token obtained via refresh)
// ============================================================================

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct EtsyOAuthConfig {
    refresh_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_at_utc_secs: Option<i64>,
}

fn etsy_config_path() -> Option<PathBuf> {
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

/// Returns a valid Etsy OAuth access token: from config (if not expired), from refresh, or legacy ETSY_SECRET.
async fn get_etsy_access_token() -> Result<String, String> {
    let mut cfg = load_etsy_config();
    let now_secs = Utc::now().timestamp();
    let expires = cfg.expires_at_utc_secs.unwrap_or(0);
    // Use cached access token if still valid (with 5 min buffer)
    if cfg.access_token.is_some() && expires > now_secs + 300 {
        return Ok(cfg.access_token.as_ref().unwrap().clone());
    }
    // Prefer OAuth refresh token over legacy ETSY_SECRET
    if let Some(ref refresh) = cfg.refresh_token {
        let refresh = refresh.clone();
        return refresh_etsy_token_async(&mut cfg, &refresh).await;
    }
    // Legacy: use ETSY_SECRET from .env if set (e.g. manual token)
    if !ETSY_SECRET.is_empty() {
        return Ok(ETSY_SECRET.to_string());
    }
    Err("Etsy not connected. Get a refresh token from order-tracker.kingsofalchemy.com/connect and paste it in Settings.".to_string())
}

async fn refresh_etsy_token_async(cfg: &mut EtsyOAuthConfig, refresh_token: &str) -> Result<String, String> {
    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", ETSY_KEYSTRING),
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

    #[derive(serde::Deserialize)]
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

/// Save a new refresh token (from web OAuth flow) and clear cached access token so next use refreshes.
pub fn save_etsy_refresh_token(refresh_token: String) -> Result<(), String> {
    let mut cfg = load_etsy_config();
    cfg.refresh_token = Some(refresh_token.trim().to_string());
    cfg.access_token = None;
    cfg.expires_at_utc_secs = None;
    save_etsy_config(&cfg)
}

pub fn has_etsy_oauth() -> bool {
    let cfg = load_etsy_config();
    cfg.refresh_token.is_some() || (!ETSY_SECRET.is_empty())
}

// ============================================================================
// Data Models
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetalType {
    Gold,
    Silver,
    Bronze,
    Unknown,
}

impl MetalType {
    fn from_string(s: &str) -> Self {
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

    fn display_class(&self) -> &'static str {
        match self {
            MetalType::Gold => "badge-gold",
            MetalType::Silver => "badge-silver",
            MetalType::Bronze => "badge-bronze",
            MetalType::Unknown => "badge-nebula",
        }
    }

    fn display_name(&self) -> &'static str {
        match self {
            MetalType::Gold => "Gold",
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
}

// ============================================================================
// Shopify API Types
// ============================================================================

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

// ============================================================================
// Etsy API Types (v3: GET /v3/application/shops/{shop_id}/receipts)
// ============================================================================

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
    // Etsy returns both create_timestamp and created_timestamp; use one to avoid duplicate-field error
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
}

#[derive(Debug, Deserialize)]
struct EtsyVariation {
    formatted_name: Option<String>,
    formatted_value: Option<String>,
}

// ============================================================================
// API Functions
// ============================================================================

async fn fetch_shopify_orders() -> Result<Vec<Order>, String> {
    let client = reqwest::Client::new();
    
    // Get orders from the last 2 months, any status
    let two_months_ago = Utc::now() - Duration::days(60);
    let created_at_min = two_months_ago.format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    
    // Use the SHOPIFY_URL env var and fetch all statuses
    let url = format!(
        "{}/orders.json?status=any&limit=250&created_at_min={}",
        SHOPIFY_URL,
        created_at_min
    );

    let response = client
        .get(&url)
        .header("X-Shopify-Access-Token", SHOPIFY_ACCESS_TOKEN)
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

    let orders = shopify_response
        .orders
        .into_iter()
        .map(|so| {
            let order_date = DateTime::parse_from_rfc3339(&so.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            // Due date is 2 weeks from order date
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

async fn fetch_etsy_orders() -> Result<Vec<Order>, String> {
    let access_token = get_etsy_access_token().await?;
    // Etsy API v3: GET https://api.etsy.com/v3/application/shops/{shop_id}/receipts
    // Requires x-api-key: {keystring}:{shared_secret} and Authorization: Bearer <oauth_token>
    let client = reqwest::Client::new();
    const LIMIT: i32 = 100;
    let base_url = format!(
        "https://api.etsy.com/v3/application/shops/{}/receipts",
        ETSY_SHOP_ID
    );

    let x_api_key = format!("{}:{}", ETSY_KEYSTRING, ETSY_SECRET);

    let mut all_receipts = Vec::new();
    let mut offset = 0i32;

    loop {
        let url = format!("{}?limit={}&offset={}", base_url, LIMIT, offset);
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
                    "Check x-api-key and OAuth token (Bearer) with transactions_r scope"
                        .to_string()
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
                eprintln!("[Etsy API raw response (offset={})]: {}", offset, preview);
                return Err(format!("Etsy response parse failed: {} | raw preview: {}", e, preview));
            }
        };

        let results = page.results;
        let n = results.len() as i32;
        all_receipts.extend(results);

        if n < LIMIT {
            break;
        }
        offset += LIMIT;
    }

    let two_months_ago = Utc::now() - Duration::days(60);
    let orders: Vec<Order> = all_receipts
        .into_iter()
        .filter_map(|r| {
            let order_ts = r.create_timestamp;
            // Etsy timestamps: can be seconds or milliseconds
            let order_date = if order_ts > 1_000_000_000_000 {
                Utc.timestamp_millis_opt(order_ts).single().unwrap_or(Utc::now())
            } else {
                Utc.timestamp_opt(order_ts, 0).single().unwrap_or(Utc::now())
            };
            if order_date < two_months_ago {
                return None;
            }
            let due_date = order_date + Duration::days(14);

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
                    let price_val = t.price.as_ref().map(|p| {
                        let div = p.divisor.unwrap_or(100).max(1) as f64;
                        (p.amount.unwrap_or(0) as f64) / div
                    }).unwrap_or(0.0);
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
                        .find(|s| s.to_lowercase().contains("ring") || s.to_lowercase().contains("size"))
                        .cloned();
                    OrderItem {
                        name: title,
                        quantity: qty as u32,
                        price: price_val,
                        metal_type,
                        ring_size,
                        variant_info,
                    }
                })
                .collect();

            let total_price = if total_price > 0.0 {
                total_price
            } else {
                items.iter().map(|i| i.price * i.quantity as f64).sum::<f64>()
            };

            let shipping_address = r
                .first_line
                .clone()
                .or(r.formatted_address.clone());

            Some(Order {
                id: r.receipt_id.to_string(),
                source: OrderSource::Etsy,
                order_number: format!(
                    "#{}",
                    r.order_id.unwrap_or(r.receipt_id)
                ),
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

    Ok(orders)
}

fn extract_ring_size(name: &str, properties: &Option<Vec<ShopifyProperty>>) -> Option<String> {
    // Check properties first (Shopify custom options)
    if let Some(props) = properties {
        for prop in props {
            let prop_name_lower = prop.name.to_lowercase();
            if prop_name_lower.contains("size") || prop_name_lower.contains("ring") {
                return Some(prop.value.clone());
            }
        }
    }

    // Try to extract from name/variant
    let lower = name.to_lowercase();
    
    // Common ring size patterns
    let patterns = [
        "size ", "ring size ", "sz ", "us ", "uk ",
    ];
    
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

// ============================================================================
// App State
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum ViewFilter {
    All,
    Shopify,
    Etsy,
    Urgent,
}

#[derive(Debug, Clone, PartialEq)]
enum SortBy {
    DueDate,
    OrderDate,
    Customer,
}

// ============================================================================
// Components
// ============================================================================

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    // State
    let mut orders = use_signal(Vec::<Order>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut view_filter = use_signal(|| ViewFilter::All);
    let mut sort_by = use_signal(|| SortBy::DueDate);
    let mut search_query = use_signal(String::new);
    let mut settings_open = use_signal(|| false);
    let mut etsy_token_input = use_signal(String::new);
    let mut etsy_save_message = use_signal(|| None::<String>);

    // Fetch orders on mount
    use_effect(move || {
        spawn(async move {
            loading.set(true);
            error.set(None);

            let mut all_orders = Vec::new();

            // Fetch Shopify orders
            match fetch_shopify_orders().await {
                Ok(shopify_orders) => {
                    all_orders.extend(shopify_orders);
                }
                Err(e) => {
                    eprintln!("Shopify error: {}", e);
                    error.set(Some(format!("Shopify: {}", e)));
                }
            }

            // Fetch Etsy orders
            match fetch_etsy_orders().await {
                Ok(etsy_orders) => {
                    all_orders.extend(etsy_orders);
                }
                Err(e) => {
                    eprintln!("Etsy error: {}", e);
                    error.set(Some(format!("Etsy: {}", e)));
                }
            }

            // Sort by due date by default
            all_orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));

            orders.set(all_orders);
            loading.set(false);
        });
    });

    // Filter and sort orders
    let filtered_orders = use_memo(move || {
        let mut result: Vec<Order> = orders
            .read()
            .iter()
            .filter(|order| {
                // Apply view filter
                let passes_filter = match *view_filter.read() {
                    ViewFilter::All => true,
                    ViewFilter::Shopify => matches!(order.source, OrderSource::Shopify),
                    ViewFilter::Etsy => matches!(order.source, OrderSource::Etsy),
                    ViewFilter::Urgent => order.days_until_due() <= 3,
                };

                // Apply search filter
                let query = search_query.read().to_lowercase();
                let passes_search = query.is_empty()
                    || order.customer_name.to_lowercase().contains(&query)
                    || order.order_number.to_lowercase().contains(&query)
                    || order.items.iter().any(|item| item.name.to_lowercase().contains(&query));

                passes_filter && passes_search
            })
            .cloned()
            .collect();

        // Apply sorting
        match *sort_by.read() {
            SortBy::DueDate => result.sort_by(|a, b| a.due_date.cmp(&b.due_date)),
            SortBy::OrderDate => result.sort_by(|a, b| b.order_date.cmp(&a.order_date)),
            SortBy::Customer => result.sort_by(|a, b| a.customer_name.cmp(&b.customer_name)),
        }

        result
    });

    // Calculate stats
    let stats = use_memo(move || {
        let all = orders.read();
        let total = all.len();
        let shopify = all.iter().filter(|o| matches!(o.source, OrderSource::Shopify)).count();
        let etsy = all.iter().filter(|o| matches!(o.source, OrderSource::Etsy)).count();
        let urgent = all.iter().filter(|o| o.days_until_due() <= 3).count();
        let overdue = all.iter().filter(|o| o.days_until_due() < 0).count();
        (total, shopify, etsy, urgent, overdue)
    });

    rsx! {
        document::Stylesheet { href: asset!("/assets/styles.css") }
        
        div { class: "bg-galaxy min-h-screen",
            // Navigation Header
            nav { class: "nav-galaxy px-6 py-4",
                div { class: "container flex items-center justify-between",
                    div { class: "flex items-center gap-4",
                        h1 { class: "text-2xl font-bold text-star-white",
                            "📦 Order Tracker"
                        }
                        div { class: "live-indicator",
                            span { class: "live-dot" }
                            span { class: "text-sm text-stardust", "Live" }
                        }
                    }
                    
                    div { class: "flex items-center gap-3",
                        button {
                            class: "btn-cosmic",
                                onclick: move |_| {
                                    loading.set(true);
                                    error.set(None);
                                    spawn(async move {
                                        let mut all_orders = Vec::new();
                                        match fetch_shopify_orders().await {
                                            Ok(shopify) => all_orders.extend(shopify),
                                            Err(e) => {
                                                eprintln!("Shopify error: {}", e);
                                                error.set(Some(format!("Shopify: {}", e)));
                                            }
                                        }
                                        match fetch_etsy_orders().await {
                                            Ok(etsy) => all_orders.extend(etsy),
                                            Err(e) => {
                                                eprintln!("Etsy error: {}", e);
                                                error.set(Some(format!("Etsy: {}", e)));
                                            }
                                        }
                                        all_orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));
                                        orders.set(all_orders);
                                        loading.set(false);
                                    });
                                },
                            "🔄 Refresh"
                        }
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| {
                                settings_open.set(true);
                                etsy_save_message.set(None);
                            },
                            "⚙️ Settings"
                        }
                    }
                }
            }

            // Settings modal
            {if *settings_open.read() {
                rsx! {
                    div {
                        class: "fixed inset-0 z-50 flex items-center justify-center bg-black/60",
                        div {
                            class: "card-cosmic p-6 max-w-lg w-full mx-4 max-h-[90vh] overflow-y-auto",
                            onclick: move |evt| { evt.stop_propagation(); },
                            h2 { class: "text-xl font-bold text-star-white mb-4", "⚙️ Settings" }
                            div { class: "space-y-4",
                                div {
                                    class: "border border-nebula-purple rounded-lg p-4",
                                    h3 { class: "text-star-white font-medium mb-2", "🧶 Connect Etsy" }
                                    p { class: "text-stardust text-sm mb-3",
                                        "Get a refresh token from the Order Tracker website, then paste it below."
                                    }
                                    a {
                                        href: "https://order-tracker.kingsofalchemy.com/connect",
                                        target: "_blank",
                                        class: "text-nebula-purple underline text-sm mb-3 block",
                                        "Get token at order-tracker.kingsofalchemy.com/connect →"
                                    }
                                    textarea {
                                        class: "w-full bg-nebula-dark border border-nebula-purple rounded-lg px-3 py-2 text-star-white font-mono text-sm min-h-[80px]",
                                        placeholder: "Paste Etsy refresh token here...",
                                        value: "{etsy_token_input}",
                                        oninput: move |evt| etsy_token_input.set(evt.value())
                                    }
                                    div { class: "flex gap-2 mt-2",
                                        button {
                                            class: "btn-nebula",
                                            onclick: move |_| {
                                                let token = etsy_token_input.read().clone();
                                                if token.trim().is_empty() {
                                                    etsy_save_message.set(Some("Enter a token first.".to_string()));
                                                    return;
                                                }
                                                match save_etsy_refresh_token(token) {
                                                    Ok(()) => {
                                                        etsy_save_message.set(Some("Etsy connected. Refresh orders to load Etsy.".to_string()));
                                                        etsy_token_input.set(String::new());
                                                    }
                                                    Err(e) => etsy_save_message.set(Some(e)),
                                                }
                                            },
                                            "Save token"
                                        }
                                    }
                                    {if let Some(msg) = etsy_save_message.read().as_ref() {
                                        rsx! { p { class: "text-sm mt-2 text-stardust", "{msg}" } }
                                    } else {
                                        rsx! { }
                                    }}
                                }
                            }
                            div { class: "mt-6 flex justify-end",
                                button {
                                    class: "btn-cosmic",
                                    onclick: move |_| settings_open.set(false),
                                    "Close"
                                }
                            }
                        }
                    }
                }
            } else {
                rsx! { }
            }}

            // Main Content
            div { class: "container px-6 py-8",
                // Stats Cards
                div { class: "stats-grid mb-8",
                    StatCard {
                        title: "Total Orders",
                        value: stats.read().0.to_string(),
                        icon: "📦"
                    }
                    StatCard {
                        title: "Shopify",
                        value: stats.read().1.to_string(),
                        icon: "🛒"
                    }
                    StatCard {
                        title: "Etsy",
                        value: stats.read().2.to_string(),
                        icon: "🧶"
                    }
                    StatCard {
                        title: "Urgent (≤3 days)",
                        value: stats.read().3.to_string(),
                        icon: "⚠️"
                    }
                    StatCard {
                        title: "Overdue",
                        value: stats.read().4.to_string(),
                        icon: "🚨"
                    }
                }

                // Filters and Search
                div { class: "card-cosmic p-6 mb-6",
                    div { class: "flex flex-wrap items-center gap-4",
                        // Search
                        div { class: "flex-1 min-w-0",
                            input {
                                r#type: "search",
                                class: "w-full",
                                placeholder: "Search orders, customers, products...",
                                value: "{search_query}",
                                oninput: move |evt| search_query.set(evt.value())
                            }
                        }

                        // Filter Buttons
                        div { class: "flex gap-2",
                            FilterButton {
                                label: "All",
                                active: *view_filter.read() == ViewFilter::All,
                                onclick: move |_| view_filter.set(ViewFilter::All)
                            }
                            FilterButton {
                                label: "Shopify",
                                active: *view_filter.read() == ViewFilter::Shopify,
                                onclick: move |_| view_filter.set(ViewFilter::Shopify)
                            }
                            FilterButton {
                                label: "Etsy",
                                active: *view_filter.read() == ViewFilter::Etsy,
                                onclick: move |_| view_filter.set(ViewFilter::Etsy)
                            }
                            FilterButton {
                                label: "🔥 Urgent",
                                active: *view_filter.read() == ViewFilter::Urgent,
                                onclick: move |_| view_filter.set(ViewFilter::Urgent)
                            }
                        }

                        // Sort Dropdown
                        div { class: "flex items-center gap-2",
                            span { class: "text-stardust text-sm", "Sort by:" }
                            select {
                                class: "bg-nebula-dark border border-nebula-purple rounded-lg px-3 py-2 text-star-white",
                                onchange: move |evt| {
                                    match evt.value().as_str() {
                                        "due" => sort_by.set(SortBy::DueDate),
                                        "order" => sort_by.set(SortBy::OrderDate),
                                        "customer" => sort_by.set(SortBy::Customer),
                                        _ => {}
                                    }
                                },
                                option { value: "due", "Due Date" }
                                option { value: "order", "Order Date" }
                                option { value: "customer", "Customer" }
                            }
                        }
                    }
                }

                // Orders List
                div { class: "card-cosmic overflow-hidden",
                    if *loading.read() {
                        div { class: "p-8 text-center",
                            div { class: "animate-pulse-glow inline-block",
                                span { class: "text-4xl", "⏳" }
                            }
                            p { class: "text-stardust mt-4", "Loading orders..." }
                        }
                    } else if filtered_orders.read().is_empty() {
                        div { class: "p-8 text-center",
                            span { class: "text-4xl", "📭" }
                            p { class: "text-stardust mt-4", "No orders found" }
                        }
                    } else {
                        div { class: "overflow-x-auto",
                            table { class: "table-cosmic",
                                thead {
                                    tr {
                                        th { "Order" }
                                        th { "Customer" }
                                        th { "Items" }
                                        th { "Metal" }
                                        th { "Size" }
                                        th { "Due Date" }
                                        th { "Days Left" }
                                        th { "Total" }
                                        th { "Source" }
                                    }
                                }
                                tbody {
                                    for order in filtered_orders.read().iter() {
                                        OrderRow { order: order.clone() }
                                    }
                                }
                            }
                        }
                    }
                }

                // Error Display
                if let Some(err) = error.read().as_ref() {
                    div { class: "card-cosmic p-4 mt-4 border-warning-red",
                        div { class: "flex items-center gap-3",
                            span { class: "text-2xl", "⚠️" }
                            p { class: "text-warning-red", "{err}" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StatCard(title: String, value: String, icon: String) -> Element {
    rsx! {
        div { class: "card-stat",
            div { class: "flex items-center justify-between",
                div {
                    p { class: "text-stardust text-sm font-medium mb-1", "{title}" }
                    p { class: "stat-value", "{value}" }
                }
                span { class: "text-4xl opacity-75", "{icon}" }
            }
        }
    }
}

#[component]
fn FilterButton(label: String, active: bool, onclick: EventHandler<MouseEvent>) -> Element {
    let class = if active {
        "btn-nebula"
    } else {
        "btn-cosmic"
    };

    rsx! {
        button {
            class: "{class}",
            onclick: move |evt| onclick.call(evt),
            "{label}"
        }
    }
}

#[component]
fn OrderRow(order: Order) -> Element {
    let days_left = order.days_until_due();
    let urgency_class = order.urgency_class();
    
    let days_display = if days_left < 0 {
        format!("{} overdue", days_left.abs())
    } else if days_left == 0 {
        "Today!".to_string()
    } else if days_left == 1 {
        "1 day".to_string()
    } else {
        format!("{} days", days_left)
    };

    let source_badge = match order.source {
        OrderSource::Shopify => ("🛒 Shopify", "badge-method"),
        OrderSource::Etsy => ("🧶 Etsy", "badge-nebula"),
    };

    // Get primary metal type and ring size from items
    let primary_metal = order
        .items
        .first()
        .map(|i| i.metal_type.clone())
        .unwrap_or(MetalType::Unknown);

    let ring_size = order
        .items
        .iter()
        .find_map(|i| i.ring_size.clone())
        .unwrap_or_else(|| "N/A".to_string());

    let items_display: Vec<String> = order
        .items
        .iter()
        .map(|i| {
            if i.quantity > 1 {
                format!("{}x {}", i.quantity, i.name)
            } else {
                i.name.clone()
            }
        })
        .collect();

    rsx! {
        tr { class: "{urgency_class}",
            td {
                div { class: "font-semibold text-star-white", "{order.order_number}" }
                div { class: "text-xs text-stardust", 
                    "{order.order_date.format(\"%b %d, %Y\")}" 
                }
            }
            td { class: "text-moonlight", "{order.customer_name}" }
            td {
                div { class: "max-w-xs",
                    for (idx, item) in items_display.iter().enumerate() {
                        div { 
                            class: "text-sm truncate",
                            class: if idx > 0 { "text-stardust" } else { "text-star-white" },
                            "{item}"
                        }
                    }
                }
            }
            td {
                {
                    let badge_class = format!("badge {}", primary_metal.display_class());
                    let metal_name = primary_metal.display_name();
                    rsx! {
                        span { class: "{badge_class}", "{metal_name}" }
                    }
                }
            }
            td { 
                span { class: "font-mono text-aurora-purple", "{ring_size}" }
            }
            td { 
                class: "text-moonlight",
                "{order.due_date.format(\"%b %d\")}"
            }
            td {
                {
                    let text_color = match urgency_class {
                        "urgency-overdue" => "font-bold text-warning-red",
                        "urgency-critical" => "font-bold text-supernova-orange",
                        "urgency-warning" => "font-bold text-comet-gold",
                        _ => "font-bold text-alien-green",
                    };
                    rsx! {
                        span { class: "{text_color}", "{days_display}" }
                    }
                }
            }
            td { 
                class: "text-star-white font-semibold",
                {format!("$ {:.2}", order.total_price)}
            }
            td {
                {
                    let source_class = format!("badge {}", source_badge.1);
                    let source_name = source_badge.0;
                    rsx! {
                        span { class: "{source_class}", "{source_name}" }
                    }
                }
            }
        }
    }
}
