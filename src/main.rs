#![allow(non_snake_case)]

use chrono::{DateTime, Duration, Utc};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

// Environment variables for API tokens
pub const ETSY_KEYSTRING: &str = env!("ETSY_KEYSTRING");
pub const ETSY_SECRET: &str = env!("ETSY_SECRET");
pub const ETSY_SHOP_ID: &str = env!("ETSY_SHOP_ID");
pub const SHOPIFY_URL: &str = env!("SHOPIFY_URL");
pub const SHOPIFY_ACCESS_TOKEN: &str = env!("SHOPIFY_ACCESS_TOKEN");

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
// Etsy API Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct EtsyReceiptsResponse {
    results: Vec<EtsyReceipt>,
    count: i32,
}

#[derive(Debug, Deserialize)]
struct EtsyReceipt {
    receipt_id: i64,
    order_id: i64,
    buyer_user_id: i64,
    name: String,
    create_timestamp: i64,
    grandtotal: EtsyMoney,
    transactions: Vec<EtsyTransaction>,
    formatted_address: Option<String>,
    status: String,
}

#[derive(Debug, Deserialize)]
struct EtsyMoney {
    amount: i64,
    divisor: i64,
    currency_code: String,
}

#[derive(Debug, Deserialize)]
struct EtsyTransaction {
    title: String,
    quantity: i32,
    price: EtsyMoney,
    variations: Option<Vec<EtsyVariation>>,
}

#[derive(Debug, Deserialize)]
struct EtsyVariation {
    property_id: i64,
    formatted_name: String,
    formatted_value: String,
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
    // Note: Etsy OAuth 2.0 requires a more complex flow
    // This is a simplified version - you may need to implement OAuth token refresh
    let client = reqwest::Client::new();
    
    // For Etsy API v3, you need your shop_id
    // First, get the shop ID (you might want to store this)
    let shop_url = "https://api.etsy.com/v3/application/users/me";
    
    let response = client
        .get(shop_url)
        .header("x-api-key", ETSY_KEYSTRING)
        .header("Authorization", format!("Bearer {}", ETSY_SECRET))
        .send()
        .await
        .map_err(|e| format!("Etsy user request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Etsy API error: {} - Make sure your OAuth token is valid", response.status()));
    }

    // For now, return empty - you'll need to implement proper OAuth flow
    // The ETSY_SECRET should be an OAuth access token, not the API secret
    Ok(vec![])
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
                }
            }

            // Fetch Etsy orders
            match fetch_etsy_orders().await {
                Ok(etsy_orders) => {
                    all_orders.extend(etsy_orders);
                }
                Err(e) => {
                    eprintln!("Etsy error: {}", e);
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
                                spawn(async move {
                                    // Re-fetch orders
                                    let mut all_orders = Vec::new();
                                    if let Ok(shopify) = fetch_shopify_orders().await {
                                        all_orders.extend(shopify);
                                    }
                                    if let Ok(etsy) = fetch_etsy_orders().await {
                                        all_orders.extend(etsy);
                                    }
                                    all_orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));
                                    orders.set(all_orders);
                                    loading.set(false);
                                });
                            },
                            "🔄 Refresh"
                        }
                    }
                }
            }

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
