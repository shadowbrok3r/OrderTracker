#![allow(non_snake_case)]

mod components;
mod db;
mod etsy;
mod log;
mod model;
mod shopify;

use dioxus::prelude::*;
use log::{app_logs_snapshot, LogEntry};

use components::dialog::{DialogContent, DialogRoot, DialogTitle};
use db::{lookup_piece_cost, ItemCostWeight, PieceCostRow};
use etsy::{fetch_etsy_orders, save_etsy_refresh_token};
use model::{MetalType, Order, OrderItem, OrderSource};
use shopify::fetch_shopify_orders;

// ============================================================================
// App state
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
// Entry & root component
// ============================================================================

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let mut orders = use_signal(Vec::<Order>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut view_filter = use_signal(|| ViewFilter::All);
    let mut sort_by = use_signal(|| SortBy::DueDate);
    let mut search_query = use_signal(String::new);
    let mut settings_open = use_signal(|| false);
    let mut etsy_token_input = use_signal(String::new);
    let mut etsy_save_message = use_signal(|| None::<String>);
    let mut detail_order = use_signal(|| None::<Order>);
    let mut logs_open = use_signal(|| false);
    let mut log_snapshot = use_signal(|| Vec::<LogEntry>::new());
    let mut piece_costs_cache = use_signal(|| Vec::<PieceCostRow>::new());

    use_effect(move || {
        spawn(async move {
            if db::init_db().await.is_ok() {
                match db::load_piece_costs(&*db::DB).await {
                    Ok(rows) => piece_costs_cache.set(rows),
                    Err(e) => log::app_log("INFO", format!("Piece costs load: {}", e)),
                }
            }
        });
    });

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            error.set(None);
            log::app_log("INFO", "Fetching orders...");
            let mut all_orders = Vec::new();

            log::app_log("INFO", "Fetching Shopify orders...");
            match fetch_shopify_orders().await {
                Ok(shopify_orders) => {
                    log::app_log("INFO", format!("Shopify: {} orders", shopify_orders.len()));
                    all_orders.extend(shopify_orders);
                }
                Err(e) => {
                    log::app_log("ERROR", format!("Shopify: {}", e));
                    error.set(Some(format!("Shopify: {}", e)));
                }
            }
            log::app_log("INFO", "Fetching Etsy orders (paid, not yet shipped)...");
            match fetch_etsy_orders().await {
                Ok(etsy_orders) => {
                    log::app_log("INFO", format!("Etsy: {} orders", etsy_orders.len()));
                    all_orders.extend(etsy_orders);
                }
                Err(e) => {
                    log::app_log("ERROR", format!("Etsy: {}", e));
                    error.set(Some(format!("Etsy: {}", e)));
                }
            }

            all_orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));
            let total = all_orders.len();
            orders.set(all_orders);
            loading.set(false);
            log::app_log("INFO", format!("Done. {} total orders.", total));
        });
    });

    let filtered_orders = use_memo(move || {
        let mut result: Vec<Order> = orders
            .read()
            .iter()
            .filter(|order| {
                let passes_filter = match *view_filter.read() {
                    ViewFilter::All => true,
                    ViewFilter::Shopify => matches!(order.source, OrderSource::Shopify),
                    ViewFilter::Etsy => matches!(order.source, OrderSource::Etsy),
                    ViewFilter::Urgent => order.days_until_due() <= 3,
                };
                let query = search_query.read().to_lowercase();
                let passes_search = query.is_empty()
                    || order.customer_name.to_lowercase().contains(&query)
                    || order.order_number.to_lowercase().contains(&query)
                    || order.items.iter().any(|item| item.name.to_lowercase().contains(&query));
                passes_filter && passes_search
            })
            .cloned()
            .collect();
        match *sort_by.read() {
            SortBy::DueDate => result.sort_by(|a, b| a.due_date.cmp(&b.due_date)),
            SortBy::OrderDate => result.sort_by(|a, b| b.order_date.cmp(&a.order_date)),
            SortBy::Customer => result.sort_by(|a, b| a.customer_name.cmp(&b.customer_name)),
        }
        result
    });

    let stats = use_memo(move || {
        let all = orders.read();
        let total = all.len();
        let shopify = all.iter().filter(|o| matches!(o.source, OrderSource::Shopify)).count();
        let etsy = all.iter().filter(|o| matches!(o.source, OrderSource::Etsy)).count();
        let urgent = all.iter().filter(|o| o.days_until_due() <= 3).count();
        let overdue = all.iter().filter(|o| o.days_until_due() < 0).count();
        (total, shopify, etsy, urgent, overdue)
    });

    // Pairs of (order to display, order to pass to detail modal) so we can clone in closure without `let` inside rsx!
    let orders_for_table = use_memo(move || {
        filtered_orders
            .read()
            .iter()
            .map(|o| (o.clone(), o.clone()))
            .collect::<Vec<(Order, Order)>>()
    });

    rsx! {
        document::Stylesheet { href: asset!("/assets/styles.css") }
        document::Stylesheet { href: asset!("/assets/dx-components-theme.css") }
        document::Stylesheet { href: asset!("/assets/dialog.css") }

        div { class: "bg-galaxy min-h-screen",
            nav { class: "nav-galaxy px-6 py-4",
                div { class: "container flex items-center justify-between flex-wrap gap-3",
                    div { class: "flex items-center gap-4",
                        h1 { class: "text-2xl font-bold text-star-white",
                            "📦 Order Tracker"
                        }
                        div { class: "live-indicator",
                            span { class: "live-dot" }
                            span { class: "text-sm text-stardust", "Live" }
                        }
                        div { class: "nav-stats text-stardust text-sm flex items-center gap-4 flex-wrap",
                            span { "📦 {stats.read().0} orders" }
                            span { "🛒 {stats.read().1} Shopify" }
                            span { "🧶 {stats.read().2} Etsy" }
                            span { "⚠️ {stats.read().3} urgent" }
                            span { "🚨 {stats.read().4} overdue" }
                        }
                    }
                    div { class: "flex items-center gap-3",
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| {
                                loading.set(true);
                                error.set(None);
                                spawn(async move {
                                    log::app_log("INFO", "Refresh: fetching orders...");
                                    let mut all_orders = Vec::new();
                                    match fetch_shopify_orders().await {
                                        Ok(shopify) => {
                                            log::app_log("INFO", format!("Shopify: {} orders", shopify.len()));
                                            all_orders.extend(shopify);
                                        }
                                        Err(e) => {
                                            log::app_log("ERROR", format!("Shopify: {}", e));
                                            error.set(Some(format!("Shopify: {}", e)));
                                        }
                                    }
                                    match fetch_etsy_orders().await {
                                        Ok(etsy) => {
                                            log::app_log("INFO", format!("Etsy: {} orders", etsy.len()));
                                            all_orders.extend(etsy);
                                        }
                                        Err(e) => {
                                            log::app_log("ERROR", format!("Etsy: {}", e));
                                            error.set(Some(format!("Etsy: {}", e)));
                                        }
                                    }
                                    all_orders.sort_by(|a, b| a.due_date.cmp(&b.due_date));
                                    let total = all_orders.len();
                                    orders.set(all_orders);
                                    loading.set(false);
                                    log::app_log("INFO", format!("Refresh done. {} total orders.", total));
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
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| {
                                logs_open.set(true);
                                log_snapshot.set(app_logs_snapshot());
                            },
                            "📋 Logs"
                        }
                    }
                }
            }

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

            DialogRoot {
                open: *logs_open.read(),
                on_open_change: move |open: bool| logs_open.set(open),
                DialogContent {
                    class: "flex flex-col max-h-[85vh]",
                    DialogTitle { "📋 Logs" }
                    p { class: "text-stardust text-sm", "App and API activity. Re-open to refresh." }
                    div { class: "flex-1 overflow-y-auto font-mono text-xs bg-nebula-dark rounded-lg p-3 border border-nebula-purple/30 min-h-[200px]",
                        for entry in log_snapshot.read().iter() {
                            div { class: "log-line py-0.5",
                                span { class: "text-stardust mr-2", "{entry.time}" }
                                span { class: if entry.level == "ERROR" { "text-warning-red font-semibold" } else { "text-aurora-purple" }, "{entry.level}" }
                                span { class: "text-moonlight ml-2", "{entry.message}" }
                            }
                        }
                    }
                    div { class: "flex gap-2 mt-4",
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| log_snapshot.set(app_logs_snapshot()),
                            "Refresh logs"
                        }
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| logs_open.set(false),
                            "Close"
                        }
                    }
                }
            }

            DialogRoot {
                open: detail_order.read().is_some(),
                on_open_change: move |open: bool| {
                    if !open {
                        detail_order.set(None);
                    }
                },
                DialogContent {
                    class: "max-w-2xl max-h-[90vh] overflow-y-auto",
                    {if let Some(order) = detail_order.read().as_ref() {
                        rsx! {
                            OrderDetailDialog {
                                order: order.clone(),
                                piece_costs: piece_costs_cache.read().clone(),
                                on_close: move |_| detail_order.set(None)
                            }
                        }
                    } else {
                        rsx! { }
                    }}
                }
            }

            div { class: "container px-6 py-6",
                div { class: "card-cosmic p-6 mb-6",
                    div { class: "flex flex-wrap items-center gap-4",
                        div { class: "flex-1 min-w-0",
                            input {
                                r#type: "search",
                                class: "w-full",
                                placeholder: "Search orders, customers, products...",
                                value: "{search_query}",
                                oninput: move |evt| search_query.set(evt.value())
                            }
                        }
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
                        div { class: "flex items-center gap-2",
                            span { class: "text-stardust text-sm", "Sort by:" }
                            select {
                                class: "bg-nebula-dark border border-nebula-purple rounded-lg px-3 py-2",
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
                            table { class: "table-cosmic table-orders",
                                thead {
                                    tr {
                                        th { class: "th-thumb", "" }
                                        th { "Order" }
                                        th { "Customer" }
                                        th { class: "th-items", "Items" }
                                        th { "Metal" }
                                        th { "Size" }
                                        th { "Due Date" }
                                        th { "Days Left" }
                                        th { "Total" }
                                        th { title: "Our cost (from catalog)", "Cost" }
                                        th { title: "Weight (g)", "Weight" }
                                        th { "Source" }
                                    }
                                }
                                tbody {
                                    for (order, order_for_click) in orders_for_table.read().clone() {
                                        OrderRow {
                                            order,
                                            piece_costs: piece_costs_cache.read().clone(),
                                            on_click: move |_| detail_order.set(Some(order_for_click.clone())),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                {if let Some(err) = error.read().as_ref() {
                    rsx! {
                        div { class: "card-cosmic p-4 mt-4 border-warning-red",
                            div { class: "flex items-center gap-3",
                                span { class: "text-2xl", "⚠️" }
                                p { class: "text-warning-red", "{err}" }
                            }
                        }
                    }
                } else {
                    rsx! { }
                }}
            }
        }
    }
}

#[component]
fn FilterButton(label: String, active: bool, onclick: EventHandler<MouseEvent>) -> Element {
    let class = if active { "btn-nebula" } else { "btn-cosmic" };
    rsx! {
        button {
            class: "{class}",
            onclick: move |evt| onclick.call(evt),
            "{label}"
        }
    }
}

#[component]
fn OrderRow(
    order: Order,
    piece_costs: Vec<PieceCostRow>,
    on_click: EventHandler<MouseEvent>,
) -> Element {
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
    let items_tooltip = items_display.join("\n");
    let first_image = order.items.first().and_then(|i| i.image_url.clone());

    let (order_cost, order_weight) = order.items.iter().fold((0.0_f64, 0.0_f64), |(c, w), item| {
        let cw = lookup_piece_cost(item, &piece_costs);
        let q = item.quantity as f64;
        (
            c + cw.as_ref().map(|x| x.cost_usd * q).unwrap_or(0.0),
            w + cw.as_ref().map(|x| x.weight_g * q).unwrap_or(0.0),
        )
    });
    let cost_str = if order_cost > 0.0 {
        format!("$ {:.2}", order_cost)
    } else {
        "—".to_string()
    };
    let weight_str = if order_weight > 0.0 {
        format!("{:.1} g", order_weight)
    } else {
        "—".to_string()
    };

    rsx! {
        tr {
            class: "{urgency_class} order-row-clickable",
            onclick: move |evt| on_click.call(evt),
            td { class: "td-thumb",
                {match first_image.as_deref() {
                    Some(url) => rsx! { img { class: "order-thumb", src: "{url}", alt: "" } },
                    None => rsx! { span { class: "order-thumb-placeholder", "📦" } },
                }}
            }
            td { class: "td-nowrap",
                div { class: "font-semibold text-star-white", "{order.order_number}" }
                div { class: "text-xs text-stardust",
                    "{order.order_date.format(\"%b %d, %Y\")}"
                }
            }
            td { class: "td-nowrap text-moonlight", title: "{order.customer_name}",
                span { class: "cell-truncate", "{order.customer_name}" }
            }
            td { class: "td-items", title: "{items_tooltip}",
                div { class: "items-cell cell-truncate",
                    for (idx, item) in items_display.iter().enumerate() {
                        div {
                            class: "text-sm",
                            class: if idx > 0 { "text-stardust" } else { "text-star-white" },
                            "{item}"
                        }
                    }
                }
            }
            td { class: "td-nowrap",
                {
                    let badge_class = format!("badge {}", primary_metal.display_class());
                    let metal_name = primary_metal.display_name();
                    rsx! {
                        span { class: "{badge_class}", "{metal_name}" }
                    }
                }
            }
            td { class: "td-nowrap",
                span { class: "font-mono text-aurora-purple", "{ring_size}" }
            }
            td { class: "td-nowrap text-moonlight",
                "{order.due_date.format(\"%b %d\")}"
            }
            td { class: "td-nowrap",
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
            td { class: "td-nowrap text-star-white font-semibold",
                {format!("$ {:.2}", order.total_price)}
            }
            td { class: "td-nowrap text-stardust", title: "Our cost (from catalog)", "{cost_str}" }
            td { class: "td-nowrap text-stardust", title: "Weight (g)", "{weight_str}" }
            td { class: "td-nowrap",
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

#[component]
fn OrderDetailDialog(
    order: Order,
    piece_costs: Vec<PieceCostRow>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let source_label = match order.source {
        OrderSource::Shopify => "🛒 Shopify",
        OrderSource::Etsy => "🧶 Etsy",
    };
    let days_left = order.days_until_due();
    let days_display = if days_left < 0 {
        format!("{} days overdue", days_left.abs())
    } else if days_left == 0 {
        "Due today".to_string()
    } else if days_left == 1 {
        "1 day left".to_string()
    } else {
        format!("{} days left", days_left)
    };
    let total_str = format!("{} {:.2}", order.currency, order.total_price);

    rsx! {
        div { class: "flex items-center justify-between mb-4",
            h2 { class: "text-xl font-bold text-star-white",
                "{order.order_number}"
            }
            div { class: "flex items-center gap-2",
                span { class: "badge badge-nebula", "{source_label}" }
                button {
                    class: "btn-cosmic text-sm",
                    onclick: move |evt| on_close.call(evt),
                    "Close"
                }
            }
        }
        {match order.source {
            OrderSource::Etsy => rsx! {
                p { class: "text-stardust text-sm mb-3",
                    "Receipt ID: {order.id}"
                }
            },
            OrderSource::Shopify => rsx! { },
        }}
        dl { class: "detail-grid",
            dt { "Customer" }
            dd { "{order.customer_name}" }
            dt { "Order date" }
            dd { "{order.order_date.format(\"%b %d, %Y\")}" }
            dt { "Ship by / Due" }
            dd { "{order.due_date.format(\"%b %d, %Y\")} ({days_display})" }
            dt { "Status" }
            dd { "{order.status}" }
            dt { "Total" }
            dd { class: "font-semibold text-star-white", "{total_str}" }
        }
        {{
            let order_cost: f64 = order.items.iter()
                .map(|item| {
                    let cw = lookup_piece_cost(item, &piece_costs);
                    (item.quantity as f64) * cw.as_ref().map(|x| x.cost_usd).unwrap_or(0.0)
                })
                .sum();
            if order_cost > 0.0 {
                let s = format!("$ {:.2}", order_cost);
                rsx! {
                    dt { "Our cost" }
                    dd { class: "font-semibold text-aurora-purple", "{s}" }
                }
            } else {
                rsx! { }
            }
        }}
        {order.shipping_address.as_ref().map(|addr| rsx! {
            div { class: "mt-4",
                p { class: "text-stardust text-sm font-medium mb-1", "Shipping address" }
                p { class: "text-moonlight text-sm", "{addr}" }
            }
        })}
        div { class: "mt-4",
            p { class: "text-stardust text-sm font-medium mb-2", "Items" }
            div { class: "space-y-3",
                for item in order.items.iter() {
                    OrderDetailItemRow {
                        item: item.clone(),
                        cost_weight: lookup_piece_cost(item, &piece_costs),
                    }
                }
            }
        }
    }
}

#[component]
fn OrderDetailItemRow(item: OrderItem, cost_weight: Option<ItemCostWeight>) -> Element {
    let price_str = format!("${:.2}", item.price);
    let (cost_str, weight_str) = match &cost_weight {
        Some(cw) => (
            format!("${:.2}", cw.cost_usd * item.quantity as f64),
            format!("{:.1} g", cw.weight_g * item.quantity as f64),
        ),
        None => ("—".to_string(), "—".to_string()),
    };
    rsx! {
        div { class: "flex items-start gap-3 p-3 rounded-lg bg-nebula-dark/50 border border-nebula-purple/20",
            {item.image_url.as_ref().map(|url| rsx! {
                img { class: "w-14 h-14 rounded object-cover flex-shrink-0", src: "{url}", alt: "" }
            }).unwrap_or(rsx! {
                div { class: "w-14 h-14 rounded bg-nebula-purple/20 flex items-center justify-center flex-shrink-0 text-2xl", "📦" }
            })}
            div { class: "min-w-0 flex-1",
                p { class: "font-medium text-star-white", "{item.name}" }
                {(item.quantity > 1).then(|| rsx! { p { class: "text-stardust text-sm", "Qty: {item.quantity}" } })}
                {item.variant_info.as_ref().map(|v| rsx! { p { class: "text-stardust text-sm", "{v}" } })}
                {item.ring_size.as_ref().map(|s| rsx! { p { class: "text-aurora-purple text-sm font-mono", "Size: {s}" } })}
                p { class: "text-moonlight text-sm", "{item.metal_type.display_name()} · {price_str}" }
                p { class: "text-stardust text-sm mt-1",
                    "Our cost: {cost_str} · Weight: {weight_str}"
                }
            }
        }
    }
}
