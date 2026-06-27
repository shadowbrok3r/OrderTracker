#![allow(non_snake_case)]

mod api;
mod components;
#[cfg(feature = "server")]
mod db;
#[cfg(feature = "server")]
mod etsy;
#[cfg(feature = "server")]
mod ha;
mod log;
mod model;
#[cfg(feature = "server")]
mod shopify;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use dioxus::prelude::*;
use log::{app_logs_snapshot, LogEntry};

use components::dialog::{DialogContent, DialogRoot, DialogTitle};
use model::{lookup_piece_cost, CatalogPiece, ItemCostWeight, MetalType, Order, OrderItem, OrderSource, PieceCostSize};

// ============================================================================
// App state
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
enum MainView {
    Orders,
    Catalog,
}

#[derive(Debug, Clone, PartialEq)]
enum ViewFilter {
    All,
    Shopify,
    Etsy,
    Urgent,
    Archived,
}

#[derive(Debug, Clone, PartialEq)]
enum SortBy {
    DueDate,
    OrderDate,
    Customer,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CatalogKind {
    All,
    Ring,
    Pendant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CatalogSort {
    Name,
    CostAsc,
    CostDesc,
    SizesDesc,
}

/// A pickable product in the New Order builder: a catalog piece or a name reused
/// from a past custom order.
#[derive(Clone, PartialEq)]
struct ProductOption {
    name: String,
    kind: String,
    sizes: Vec<String>,
    image_url: Option<String>,
    catalog: bool,
}

/// One in-progress line item in the New Order builder.
#[derive(Clone, PartialEq)]
struct DraftLine {
    name: String,
    kind: String,
    catalog: bool,
    sizes: Vec<String>,
    size: String,
    metal: String,
    qty: u32,
    price: String,
    image_url: Option<String>,
}

// ============================================================================
// Entry & root component
// ============================================================================

fn main() {
    #[cfg(feature = "server")]
    {
        server_main();
        return;
    }

    #[cfg(all(not(feature = "server"), target_arch = "wasm32"))]
    init_ha_ingress_server_url_for_fullstack();

    #[cfg(not(feature = "server"))]
    dioxus::launch(App);
}

// Server entry: own tokio runtime, RUST_LOG tracing subscriber, per-request
// TraceLayer logging, X-Ingress-Path SSR rewrite, and IP/PORT bind.
#[cfg(feature = "server")]
fn server_main() {
    use dioxus::server::axum::{
        self,
        body::{to_bytes, Body},
        extract::{Path, Request},
        http::{header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE}, StatusCode, Uri},
        middleware::{self, Next},
        response::Response,
        routing::get,
        Router, ServiceExt,
    };
    use dioxus::server::{DioxusRouterExt, ServeConfig};
    use tower::Layer;
    use tower_http::trace::TraceLayer;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=debug")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Strip the per-session HA ingress prefix from the request path so the inner
    // router (static assets + server fns) matches normally. No-op when HA already
    // stripped it or there is no ingress header (direct access).
    async fn strip_ingress_prefix(mut req: Request, next: Next) -> Response {
        // Force uncompressed responses so the asset-URL rewrite can edit bodies.
        req.headers_mut().remove(ACCEPT_ENCODING);
        if let Some(prefix) = req
            .headers()
            .get("x-ingress-path")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
        {
            let path = req.uri().path();
            if let Some(rest) = path.strip_prefix(&prefix) {
                let rest = if rest.is_empty() { "/" } else { rest };
                let pq = match req.uri().query() {
                    Some(q) => format!("{rest}?{q}"),
                    None => rest.to_string(),
                };
                if let Ok(uri) = pq.parse::<Uri>() {
                    tracing::debug!(from = %path, to = %rest, "strip ingress prefix");
                    *req.uri_mut() = uri;
                }
            }
        }
        next.run(req).await
    }

    // Rewrite absolute asset/api URLs in SSR HTML, the wasm-loader JS, and CSS so
    // the browser requests them through the per-session HA ingress path.
    async fn rewrite_ingress_assets(req: Request, next: Next) -> Response {
        let ingress = req
            .headers()
            .get("x-ingress-path")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty());

        let path = req.uri().path().to_string();
        let res = next.run(req).await;

        let Some(ingress) = ingress else {
            return res;
        };
        // Skip compressed bodies (Accept-Encoding is stripped upstream, but be safe).
        if res.headers().contains_key(CONTENT_ENCODING) {
            return res;
        }
        let ctype = res
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        // Detect by request path extension (content-type from ServeFile is unreliable).
        let is_js = path.ends_with(".js");
        let is_css = path.ends_with(".css");
        let is_html = !is_js && !is_css && ctype.starts_with("text/html");
        if !(is_html || is_js || is_css) {
            return res;
        }

        let (mut parts, body) = res.into_parts();
        let bytes = match to_bytes(body, usize::MAX).await {
            Ok(b) => b,
            Err(_) => return Response::from_parts(parts, Body::empty()),
        };
        let text = String::from_utf8_lossy(&bytes);
        let rewritten = if is_html {
            // The ingress prefix starts with /api/, so the /api/ and base-href
            // rewrites would re-match each other's output: run path rewrites
            // first, inject the base href last.
            text.replace("=\"/api/", &format!("=\"{ingress}/api/"))
                .replace("=\"/./assets/", &format!("=\"{ingress}/assets/"))
                .replace("=\"/assets/", &format!("=\"{ingress}/assets/"))
                .replacen("<head>", &format!("<head><base href=\"{ingress}/\">"), 1)
        } else {
            // JS and CSS: prefix every absolute asset path (incl. the wasm loader's).
            text.replace("/./assets/", &format!("{ingress}/assets/"))
                .replace("\"/assets/", &format!("\"{ingress}/assets/"))
                .replace("'/assets/", &format!("'{ingress}/assets/"))
                .replace("(/assets/", &format!("({ingress}/assets/"))
        };
        tracing::debug!(path = %path, ctype = %ctype, "rewrote ingress asset urls");

        parts.headers.remove(CONTENT_LENGTH);
        Response::from_parts(parts, Body::from(rewritten))
    }

    // Stream a cached thumbnail from the SurrealDB bucket.
    async fn thumb_handler(Path(key): Path<String>) -> Response {
        match crate::db::get_thumbnail(&key).await {
            Some((bytes, ctype)) => {
                let mut resp = Response::new(Body::from(bytes));
                if let Ok(v) = ctype.parse() {
                    resp.headers_mut().insert(CONTENT_TYPE, v);
                }
                resp
            }
            None => {
                let mut resp = Response::new(Body::empty());
                *resp.status_mut() = StatusCode::NOT_FOUND;
                resp
            }
        }
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(async {
            // Honors IP/PORT env (the run script sets 0.0.0.0:8099).
            let addr = dioxus::cli_config::fullstack_address_or_localhost();

            // Keep HA sensors current: refresh from Shopify/Etsy + push every 6h.
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                loop {
                    if crate::db::ensure_db_init().await.is_ok() {
                        let result = crate::api::live_fetch_and_cache().await;
                        crate::ha::push_orders(&result.orders).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
                }
            });

            let app: Router = Router::new()
                .serve_dioxus_application(ServeConfig::new(), App)
                .route("/thumb/{key}", get(thumb_handler))
                .layer(middleware::from_fn(rewrite_ingress_assets))
                .layer(TraceLayer::new_for_http());

            // Wrap the whole Router so strip runs before routing; Router::layer runs after.
            let app = middleware::from_fn(strip_ingress_prefix).layer(app);

            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));
            tracing::info!(%addr, "OrderTracker server listening");

            axum::serve(listener, app.into_make_service())
                .await
                .expect("axum server error");
        });
}

/// Home Assistant serves the add-on UI under a per-session ingress path
/// (`/api/hassio_ingress/<token>/`). Point dioxus server-function calls there so the
/// client's POSTs route back through ingress instead of hitting the host root.
#[cfg(target_arch = "wasm32")]
fn init_ha_ingress_server_url_for_fullstack() {
    use dioxus::fullstack::set_server_url;

    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(pathname) = window.location().pathname() else {
        return;
    };
    let Ok(origin) = window.location().origin() else {
        return;
    };

    let segments: Vec<&str> = pathname.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() >= 3 && segments[0] == "api" && segments[1] == "hassio_ingress" {
        let token = segments[2];
        let base = format!("{origin}/api/hassio_ingress/{token}");
        let leaked: &'static str = Box::leak(base.into_boxed_str());
        set_server_url(leaked);
    }
}

#[component]
fn App() -> Element {
    let mut orders = use_signal(Vec::<Order>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut view_filter = use_signal(|| ViewFilter::All);
    let mut main_view = use_signal(|| MainView::Orders);
    let mut sort_by = use_signal(|| SortBy::DueDate);
    let mut search_query = use_signal(String::new);
    let mut settings_open = use_signal(|| false);
    let mut etsy_token_input = use_signal(String::new);
    let mut etsy_save_message = use_signal(|| None::<String>);
    let mut detail_order = use_signal(|| None::<Order>);
    let mut logs_open = use_signal(|| false);
    let mut log_snapshot = use_signal(|| Vec::<LogEntry>::new());
    let mut catalog = use_signal(|| Vec::<CatalogPiece>::new());
    let mut custom_open = use_signal(|| false);
    let mut cf_customer = use_signal(String::new);
    let mut cf_search = use_signal(String::new);
    let mut cf_lines = use_signal(Vec::<DraftLine>::new);
    let mut cf_due = use_signal(String::new);
    let mut cf_msg = use_signal(|| None::<String>);

    use_effect(move || {
        spawn(async move {
            match api::fetch_catalog().await {
                Ok(rows) => catalog.set(rows),
                Err(e) => log::app_log("INFO", format!("Catalog load: {}", e)),
            }
        });
    });

    use_effect(move || {
        spawn(async move {
            loading.set(true);
            error.set(None);
            log::app_log("INFO", "Fetching orders...");
            match api::fetch_all_orders().await {
                Ok(result) => {
                    let total = result.orders.len();
                    log::app_log("INFO", format!("Got {} total orders.", total));
                    for err in &result.errors {
                        log::app_log("ERROR", err.clone());
                    }
                    if let Some(first_err) = result.errors.first() {
                        error.set(Some(first_err.clone()));
                    }
                    orders.set(result.orders);
                }
                Err(e) => {
                    log::app_log("ERROR", format!("Fetch failed: {}", e));
                    error.set(Some(e.to_string()));
                }
            }
            loading.set(false);
        });
    });

    let filtered_orders = use_memo(move || {
        let mut result: Vec<Order> = orders
            .read()
            .iter()
            .filter(|order| {
                let passes_filter = match *view_filter.read() {
                    ViewFilter::All => !order.archived,
                    ViewFilter::Shopify => !order.archived && matches!(order.source, OrderSource::Shopify),
                    ViewFilter::Etsy => !order.archived && matches!(order.source, OrderSource::Etsy),
                    ViewFilter::Urgent => !order.archived && order.days_until_due() <= 3,
                    ViewFilter::Archived => order.archived,
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

    // Total silver to buy: catalog weight for every open (non-archived, non-completed)
    // non-bronze item (gold-plated pieces are cast in silver too).
    let silver_needed = use_memo(move || {
        let cat = catalog.read();
        orders
            .read()
            .iter()
            .filter(|o| !o.archived && !o.completed)
            .flat_map(|o| o.items.iter())
            .filter(|item| item.metal_type != MetalType::Bronze)
            .filter_map(|item| {
                lookup_piece_cost(item, &cat).map(|cw| cw.weight_g * item.quantity as f64)
            })
            .sum::<f64>()
    });

    let orders_for_table = use_memo(move || {
        filtered_orders
            .read()
            .iter()
            .map(|o| (o.clone(), o.clone()))
            .collect::<Vec<(Order, Order)>>()
    });

    // Products pickable in the New Order builder: every catalog piece, plus any
    // distinct product name reused from a past custom order. Each carries a
    // representative thumbnail from a prior order so re-orders show an image.
    let product_options = use_memo(move || {
        let cat = catalog.read();
        let ords = orders.read();
        let mut opts: Vec<ProductOption> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in cat.iter() {
            let key = compact(&p.name);
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            let mut sizes: Vec<String> =
                p.sizes.iter().filter_map(|s| s.ring_size.clone()).collect();
            sizes.sort_by(|a, b| {
                ring_num(&Some(a.clone()))
                    .partial_cmp(&ring_num(&Some(b.clone())))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            sizes.dedup();
            opts.push(ProductOption {
                image_url: find_thumb(&ords, &p.name, &None),
                name: p.name.clone(),
                kind: p.kind.clone(),
                sizes,
                catalog: true,
            });
        }
        for o in ords.iter() {
            if !matches!(o.source, OrderSource::Custom) {
                continue;
            }
            for it in o.items.iter() {
                let key = compact(&it.name);
                if key.is_empty() || !seen.insert(key) {
                    continue;
                }
                opts.push(ProductOption {
                    name: it.name.clone(),
                    kind: String::new(),
                    sizes: Vec::new(),
                    image_url: it.image_url.clone(),
                    catalog: false,
                });
            }
        }
        opts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        opts
    });

    let product_filtered = use_memo(move || {
        let q = cf_search.read().to_lowercase();
        product_options
            .read()
            .iter()
            .filter(|o| q.is_empty() || o.name.to_lowercase().contains(&q))
            .cloned()
            .collect::<Vec<ProductOption>>()
    });

    let draft_total = use_memo(move || {
        cf_lines
            .read()
            .iter()
            .map(|l| l.price.trim().parse::<f64>().unwrap_or(0.0) * l.qty.max(1) as f64)
            .sum::<f64>()
    });

    let silver_txt = format!("{:.0} g Ag to buy", silver_needed());

    rsx! {
        document::Stylesheet { href: asset!("/assets/styles.css") }
        document::Stylesheet { href: asset!("/assets/dx-components-theme.css") }
        document::Stylesheet { href: asset!("/assets/dialog.css") }

        div { class: "bg-galaxy min-h-screen",
            nav { class: "nav-galaxy px-6 py-4",
                div { class: "container flex items-center justify-between flex-wrap gap-3",
                    div { class: "flex items-center gap-4",
                        h1 { class: "text-2xl font-bold text-star-white",
                            "Order Tracker"
                        }
                        div { class: "live-indicator",
                            span { class: "live-dot" }
                            span { class: "text-sm text-stardust", "Live" }
                        }
                        div { class: "nav-stats text-stardust text-sm flex items-center gap-4 flex-wrap",
                            span { "{stats.read().0} orders" }
                            span { "{stats.read().1} Shopify" }
                            span { "{stats.read().2} Etsy" }
                            span { "{stats.read().3} urgent" }
                            span { "{stats.read().4} overdue" }
                            span { class: "text-comet-gold font-semibold", "{silver_txt}" }
                        }
                    }
                    div { class: "flex items-center gap-3",
                        FilterButton {
                            label: "Orders",
                            active: *main_view.read() == MainView::Orders,
                            onclick: move |_| main_view.set(MainView::Orders)
                        }
                        FilterButton {
                            label: "Catalog",
                            active: *main_view.read() == MainView::Catalog,
                            onclick: move |_| main_view.set(MainView::Catalog)
                        }
                        button {
                            class: "btn-nebula",
                            onclick: move |_| { custom_open.set(true); cf_msg.set(None); },
                            "New order"
                        }
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| {
                                loading.set(true);
                                error.set(None);
                                spawn(async move {
                                    log::app_log("INFO", "Refresh: pulling live from Shopify + Etsy...");
                                    match api::refresh_orders().await {
                                        Ok(result) => {
                                            let total = result.orders.len();
                                            log::app_log("INFO", format!("Refresh done. {} total orders.", total));
                                            for err in &result.errors {
                                                log::app_log("ERROR", err.clone());
                                            }
                                            if let Some(first_err) = result.errors.first() {
                                                error.set(Some(first_err.clone()));
                                            }
                                            orders.set(result.orders);
                                        }
                                        Err(e) => {
                                            log::app_log("ERROR", format!("Refresh error: {}", e));
                                            error.set(Some(e.to_string()));
                                        }
                                    }
                                    loading.set(false);
                                });
                            },
                            "Refresh"
                        }
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| {
                                settings_open.set(true);
                                etsy_save_message.set(None);
                            },
                            "Settings"
                        }
                        button {
                            class: "btn-cosmic",
                            onclick: move |_| {
                                logs_open.set(true);
                                log_snapshot.set(app_logs_snapshot());
                            },
                            "Logs"
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
                            h2 { class: "text-xl font-bold text-star-white mb-4", "Settings" }
                            div { class: "space-y-4",
                                div {
                                    class: "border border-nebula-purple rounded-lg p-4",
                                    h3 { class: "text-star-white font-medium mb-2", "Connect Etsy" }
                                    p { class: "text-stardust text-sm mb-3",
                                        "Get a refresh token from the Order Tracker website, then paste it below."
                                    }
                                    a {
                                        href: "https://order-tracker.kingsofalchemy.com/connect",
                                        target: "_blank",
                                        class: "text-nebula-purple underline text-sm mb-3 block",
                                        "Get token at order-tracker.kingsofalchemy.com/connect"
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
                                                spawn(async move {
                                                    match api::save_etsy_token(token).await {
                                                        Ok(()) => {
                                                            etsy_save_message.set(Some("Etsy connected. Refresh orders to load Etsy.".to_string()));
                                                            etsy_token_input.set(String::new());
                                                        }
                                                        Err(e) => etsy_save_message.set(Some(e.to_string())),
                                                    }
                                                });
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

            {if *custom_open.read() {
                rsx! {
                    div { class: "fixed inset-0 z-50 flex items-center justify-center bg-black/60",
                        div {
                            class: "card-cosmic p-6 max-w-lg w-full mx-4 max-h-[90vh] overflow-y-auto",
                            onclick: move |evt| { evt.stop_propagation(); },
                            h2 { class: "text-xl font-bold text-star-white mb-4", "New order" }
                            div { class: "space-y-3",
                                div { class: "flex gap-2",
                                    input {
                                        r#type: "text", class: "w-full", placeholder: "Customer name",
                                        value: "{cf_customer}", oninput: move |e| cf_customer.set(e.value())
                                    }
                                    input {
                                        r#type: "date", class: "w-full",
                                        value: "{cf_due}", oninput: move |e| cf_due.set(e.value())
                                    }
                                }

                                div {
                                    input {
                                        r#type: "search", class: "w-full",
                                        placeholder: "Search a product to add (Marines MC, Hades, ...)",
                                        value: "{cf_search}", oninput: move |e| cf_search.set(e.value())
                                    }
                                    div { class: "product-picker mt-2",
                                        for opt in product_filtered.read().iter().cloned() {
                                            button {
                                                class: "product-option", r#type: "button",
                                                onclick: {
                                                    let opt = opt.clone();
                                                    move |_| {
                                                        let o = opt.clone();
                                                        let default_size = o.sizes.first().cloned().unwrap_or_default();
                                                        cf_lines.write().push(DraftLine {
                                                            name: o.name,
                                                            kind: o.kind,
                                                            catalog: o.catalog,
                                                            sizes: o.sizes,
                                                            size: default_size,
                                                            metal: "Silver".to_string(),
                                                            qty: 1,
                                                            price: String::new(),
                                                            image_url: o.image_url,
                                                        });
                                                    }
                                                },
                                                {opt.image_url.as_ref().map(|url| rsx! {
                                                    img { class: "order-thumb", src: "{url}", alt: "" }
                                                })}
                                                span { class: "po-name", "{opt.name}" }
                                                span { class: "po-meta",
                                                    {if opt.catalog {
                                                        if opt.sizes.is_empty() { opt.kind.clone() }
                                                        else { format!("{} \u{00b7} {} sizes", opt.kind, opt.sizes.len()) }
                                                    } else { "custom".to_string() }}
                                                }
                                            }
                                        }
                                    }
                                    button {
                                        class: "btn-cosmic mt-2", r#type: "button",
                                        onclick: move |_| cf_lines.write().push(DraftLine {
                                            name: String::new(), kind: String::new(), catalog: false,
                                            sizes: Vec::new(), size: String::new(), metal: "Silver".to_string(),
                                            qty: 1, price: String::new(), image_url: None,
                                        }),
                                        "+ Custom item"
                                    }
                                }

                                {if cf_lines.read().is_empty() {
                                    rsx! { p { class: "text-stardust text-sm", "No items yet \u{2014} pick a product or add a custom item." } }
                                } else {
                                    rsx! {
                                        div { class: "space-y-3",
                                            for (i, line) in cf_lines.read().iter().cloned().enumerate() {
                                                DraftLineRow { index: i, line, lines: cf_lines }
                                            }
                                        }
                                    }
                                }}

                                div { class: "flex items-center justify-between mt-2",
                                    span { class: "text-stardust text-sm", "Total charge" }
                                    span { class: "text-star-white font-semibold", {format!("$ {:.2}", draft_total())} }
                                }

                                {if let Some(m) = cf_msg.read().as_ref() {
                                    rsx! { p { class: "text-warning-red text-sm", "{m}" } }
                                } else { rsx! {} }}

                                div { class: "flex gap-2 mt-2 justify-end",
                                    button { class: "btn-cosmic", onclick: move |_| custom_open.set(false), "Cancel" }
                                    button {
                                        class: "btn-nebula",
                                        onclick: move |_| {
                                            let customer = cf_customer.read().trim().to_string();
                                            let lines = cf_lines.read().clone();
                                            if customer.is_empty() {
                                                cf_msg.set(Some("Customer name is required.".to_string()));
                                                return;
                                            }
                                            if lines.is_empty() {
                                                cf_msg.set(Some("Add at least one product.".to_string()));
                                                return;
                                            }
                                            if lines.iter().any(|l| l.name.trim().is_empty()) {
                                                cf_msg.set(Some("Every item needs a name.".to_string()));
                                                return;
                                            }
                                            let due = NaiveDate::parse_from_str(cf_due.read().trim(), "%Y-%m-%d")
                                                .ok()
                                                .and_then(|d| d.and_hms_opt(0, 0, 0))
                                                .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
                                                .unwrap_or_else(|| Utc::now() + Duration::days(14));
                                            let ords = orders.read().clone();
                                            let items: Vec<OrderItem> = lines.iter().map(|l| {
                                                let size = normalize_size(&l.size, l.catalog);
                                                let price = l.price.trim().parse::<f64>().unwrap_or(0.0);
                                                let metal = MetalType::from_string(&l.metal);
                                                let image_url = l.image_url.clone()
                                                    .or_else(|| find_thumb(&ords, &l.name, &size));
                                                OrderItem {
                                                    name: l.name.trim().to_string(),
                                                    quantity: l.qty.max(1),
                                                    price,
                                                    metal_type: metal,
                                                    ring_size: size,
                                                    variant_info: None,
                                                    image_url,
                                                }
                                            }).collect();
                                            let total: f64 = items.iter().map(|i| i.price * i.quantity as f64).sum();
                                            let order = Order {
                                                id: String::new(),
                                                source: OrderSource::Custom,
                                                order_number: "Custom".to_string(),
                                                customer_name: customer,
                                                items,
                                                order_date: Utc::now(),
                                                due_date: due,
                                                total_price: total,
                                                currency: "USD".to_string(),
                                                status: "open".to_string(),
                                                shipping_address: None,
                                                archived: false,
                                                completed: false,
                                            };
                                            spawn(async move {
                                                match api::create_custom_order(order).await {
                                                    Ok(()) => {
                                                        custom_open.set(false);
                                                        cf_customer.set(String::new());
                                                        cf_search.set(String::new());
                                                        cf_lines.set(Vec::new());
                                                        cf_due.set(String::new());
                                                        cf_msg.set(None);
                                                        if let Ok(result) = api::fetch_all_orders().await {
                                                            orders.set(result.orders);
                                                        }
                                                    }
                                                    Err(e) => cf_msg.set(Some(e.to_string())),
                                                }
                                            });
                                        },
                                        "Create order"
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                rsx! {}
            }}

            DialogRoot {
                open: *logs_open.read(),
                on_open_change: move |open: bool| logs_open.set(open),
                DialogContent {
                    class: "flex flex-col max-h-[85vh]",
                    DialogTitle { "Logs" }
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
                    class: "max-w-2xl flex flex-col max-h-[90vh]",
                    {if let Some(order) = detail_order.read().as_ref() {
                        rsx! {
                            OrderDetailDialog {
                                order: order.clone(),
                                catalog: catalog.read().clone(),
                                on_close: move |_| detail_order.set(None),
                                on_set_state: move |(key, archived, completed): (String, bool, bool)| {
                                    let k = key.clone();
                                    spawn(async move {
                                        if let Err(e) = api::set_order_state(k, archived, completed).await {
                                            log::app_log("ERROR", format!("Set order state: {}", e));
                                        }
                                    });
                                    {
                                        let mut os = orders.write();
                                        if let Some(o) = os.iter_mut().find(|o| o.state_key() == key) {
                                            o.archived = archived;
                                            o.completed = completed;
                                        }
                                    }
                                    detail_order.set(None);
                                },
                                on_set_charge: move |(id, total): (String, f64)| {
                                    let mut updated: Option<Order> = None;
                                    {
                                        let mut os = orders.write();
                                        if let Some(o) = os.iter_mut()
                                            .find(|o| matches!(o.source, OrderSource::Custom) && o.id == id)
                                        {
                                            o.total_price = total;
                                            updated = Some(o.clone());
                                        }
                                    }
                                    if let Some(o) = updated {
                                        detail_order.set(Some(o.clone()));
                                        spawn(async move {
                                            if let Err(e) = api::update_custom_order(o).await {
                                                log::app_log("ERROR", format!("Update custom order: {}", e));
                                            }
                                        });
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! { }
                    }}
                }
            }

            div { class: if *main_view.read() == MainView::Orders { "container px-6 py-6" } else { "hidden" },
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
                                label: "Urgent",
                                active: *view_filter.read() == ViewFilter::Urgent,
                                onclick: move |_| view_filter.set(ViewFilter::Urgent)
                            }
                            FilterButton {
                                label: "Archived",
                                active: *view_filter.read() == ViewFilter::Archived,
                                onclick: move |_| view_filter.set(ViewFilter::Archived)
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
                                span { class: "text-4xl", "..." }
                            }
                            p { class: "text-stardust mt-4", "Loading orders..." }
                        }
                    } else if filtered_orders.read().is_empty() {
                        div { class: "p-8 text-center",
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
                                        th { title: "Sale price minus our cost", "Margin" }
                                        th { title: "Weight (g)", "Weight" }
                                        th { "Source" }
                                    }
                                }
                                tbody {
                                    for (order, order_for_click) in orders_for_table.read().clone() {
                                        OrderRow {
                                            order,
                                            catalog: catalog.read().clone(),
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
                                p { class: "text-warning-red", "{err}" }
                            }
                        }
                    }
                } else {
                    rsx! { }
                }}
            }
            div { class: if *main_view.read() == MainView::Catalog { "container px-6 py-6" } else { "hidden" },
                CatalogView { catalog: catalog.read().clone() }
            }
        }
    }
}

fn ring_num(s: &Option<String>) -> f64 {
    s.as_deref()
        .map(|x| {
            let digits: String = x.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
            digits.trim_matches('.').parse().unwrap_or(0.0)
        })
        .unwrap_or(0.0)
}

/// Lowercased alphanumeric-only form for loose product-name matching.
fn compact(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Best existing thumbnail for a product name + size, so re-orders inherit the
/// thumbnail cached on a prior matching order. Prefers an exact size match.
fn find_thumb(orders: &[Order], name: &str, size: &Option<String>) -> Option<String> {
    let nk = compact(name);
    if nk.is_empty() {
        return None;
    }
    let want = ring_num(size);
    let mut fallback = None;
    for o in orders {
        for it in &o.items {
            let Some(img) = it.image_url.as_ref() else { continue };
            let ik = compact(&it.name);
            if ik.is_empty() || !(ik == nk || ik.contains(&nk) || nk.contains(&ik)) {
                continue;
            }
            if fallback.is_none() {
                fallback = Some(img.clone());
            }
            if want > 0.0 && (ring_num(&it.ring_size) - want).abs() < 0.01 {
                return Some(img.clone());
            }
        }
    }
    fallback
}

/// Cheapest silver cost across a piece's sizes (representative for sorting).
fn piece_min_silver(p: &CatalogPiece) -> Option<f64> {
    p.sizes
        .iter()
        .filter_map(|s| s.silver_usd)
        .fold(None, |acc, v| Some(acc.map_or(v, |a: f64| a.min(v))))
}

/// Catalog sizes are stored already-formatted ("US 9"); custom free-text sizes
/// are normalized to "{n} US" when numeric, else kept verbatim.
fn normalize_size(raw: &str, catalog: bool) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if catalog {
        return Some(s.to_string());
    }
    model::format_ring_size(s).or_else(|| Some(s.to_string()))
}

#[component]
fn CatalogView(catalog: Vec<CatalogPiece>) -> Element {
    let mut search = use_signal(String::new);
    let mut kind_filter = use_signal(|| CatalogKind::All);
    let mut sort_by = use_signal(|| CatalogSort::Name);
    let mut grouped = use_signal(|| true);

    let total_pieces = catalog.len();

    let filtered: Vec<CatalogPiece> = {
        let q = search.read().to_lowercase();
        let kf = *kind_filter.read();
        let mut list: Vec<CatalogPiece> = catalog
            .iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q))
            .filter(|p| match kf {
                CatalogKind::All => true,
                CatalogKind::Ring => p.kind.eq_ignore_ascii_case("ring"),
                CatalogKind::Pendant => p.kind.eq_ignore_ascii_case("pendant"),
            })
            .cloned()
            .collect();
        match *sort_by.read() {
            CatalogSort::Name => {
                list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }
            CatalogSort::CostAsc => list.sort_by(|a, b| {
                piece_min_silver(a)
                    .unwrap_or(f64::INFINITY)
                    .partial_cmp(&piece_min_silver(b).unwrap_or(f64::INFINITY))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            CatalogSort::CostDesc => list.sort_by(|a, b| {
                piece_min_silver(b)
                    .unwrap_or(f64::NEG_INFINITY)
                    .partial_cmp(&piece_min_silver(a).unwrap_or(f64::NEG_INFINITY))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            CatalogSort::SizesDesc => list.sort_by(|a, b| b.sizes.len().cmp(&a.sizes.len())),
        }
        list
    };
    let shown = filtered.len();
    let total_rows: usize = filtered.iter().map(|p| p.sizes.len()).sum();

    let is_grouped = *grouped.read();
    let groups: Vec<(String, Vec<CatalogPiece>)> = if is_grouped {
        let pick = |k: &str| -> Vec<CatalogPiece> {
            filtered.iter().filter(|p| p.kind.eq_ignore_ascii_case(k)).cloned().collect()
        };
        let rings = pick("ring");
        let pendants = pick("pendant");
        let other: Vec<CatalogPiece> = filtered
            .iter()
            .filter(|p| !p.kind.eq_ignore_ascii_case("ring") && !p.kind.eq_ignore_ascii_case("pendant"))
            .cloned()
            .collect();
        let mut g = Vec::new();
        if !rings.is_empty() {
            g.push(("Rings".to_string(), rings));
        }
        if !pendants.is_empty() {
            g.push(("Pendants".to_string(), pendants));
        }
        if !other.is_empty() {
            g.push(("Other".to_string(), other));
        }
        g
    } else {
        vec![(String::new(), filtered.clone())]
    };

    rsx! {
        div { class: "card-cosmic p-6 mb-6",
            div { class: "flex items-center justify-between flex-wrap gap-3 mb-4",
                h2 { class: "text-xl font-bold text-star-white", "Catalog" }
                span { class: "text-stardust text-sm", "{shown} of {total_pieces} pieces \u{00b7} {total_rows} cost rows" }
            }
            div { class: "flex flex-wrap items-center gap-4",
                div { class: "flex-1 min-w-0",
                    input {
                        r#type: "search", class: "w-full",
                        placeholder: "Search catalog...",
                        value: "{search}",
                        oninput: move |e| search.set(e.value())
                    }
                }
                div { class: "flex gap-2",
                    FilterButton {
                        label: "All",
                        active: *kind_filter.read() == CatalogKind::All,
                        onclick: move |_| kind_filter.set(CatalogKind::All)
                    }
                    FilterButton {
                        label: "Rings",
                        active: *kind_filter.read() == CatalogKind::Ring,
                        onclick: move |_| kind_filter.set(CatalogKind::Ring)
                    }
                    FilterButton {
                        label: "Pendants",
                        active: *kind_filter.read() == CatalogKind::Pendant,
                        onclick: move |_| kind_filter.set(CatalogKind::Pendant)
                    }
                }
                div { class: "flex items-center gap-2",
                    span { class: "text-stardust text-sm", "Sort:" }
                    select {
                        class: "bg-nebula-dark border border-nebula-purple rounded-lg px-3 py-2",
                        onchange: move |e| {
                            match e.value().as_str() {
                                "name" => sort_by.set(CatalogSort::Name),
                                "cost_asc" => sort_by.set(CatalogSort::CostAsc),
                                "cost_desc" => sort_by.set(CatalogSort::CostDesc),
                                "sizes" => sort_by.set(CatalogSort::SizesDesc),
                                _ => {}
                            }
                        },
                        option { value: "name", "Name (A\u{2013}Z)" }
                        option { value: "cost_asc", "Silver $ (low\u{2013}high)" }
                        option { value: "cost_desc", "Silver $ (high\u{2013}low)" }
                        option { value: "sizes", "Most sizes" }
                    }
                }
                FilterButton {
                    label: "Grouped",
                    active: is_grouped,
                    onclick: move |_| { let g = *grouped.read(); grouped.set(!g); }
                }
            }
        }
        if catalog.is_empty() {
            div { class: "card-cosmic p-8 text-center",
                p { class: "text-stardust", "Catalog is empty. Publish from the cost calculator." }
            }
        } else if shown == 0 {
            div { class: "card-cosmic p-8 text-center",
                p { class: "text-stardust", "No catalog pieces match your filters." }
            }
        } else {
            for (label, pieces) in groups.iter() {
                div { class: "card-cosmic overflow-hidden mb-6",
                    {(!label.is_empty()).then(|| rsx! {
                        div { class: "catalog-group-title",
                            h3 { class: "text-star-white font-semibold", "{label}" }
                            span { class: "text-stardust text-sm", "{pieces.len()} pieces" }
                        }
                    })}
                    for piece in pieces.iter() {
                        CatalogPieceCard { piece: piece.clone() }
                    }
                }
            }
        }
    }
}

#[component]
fn CatalogPieceCard(piece: CatalogPiece) -> Element {
    let kind_class = if piece.kind == "ring" { "badge badge-nebula" } else { "badge badge-method" };
    let n = piece.sizes.len();
    let mut sizes = piece.sizes.clone();
    sizes.sort_by(|a, b| ring_num(&a.ring_size).partial_cmp(&ring_num(&b.ring_size)).unwrap_or(std::cmp::Ordering::Equal));
    let silvers: Vec<f64> = piece.sizes.iter().filter_map(|s| s.silver_usd).collect();
    let range = if silvers.is_empty() {
        "\u{2014}".to_string()
    } else {
        let lo = silvers.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = silvers.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if (hi - lo).abs() < 0.005 {
            format!("Ag $ {:.2}", lo)
        } else {
            format!("Ag $ {:.0}\u{2013}{:.0}", lo, hi)
        }
    };
    rsx! {
        details { class: "catalog-piece",
            summary { class: "catalog-summary",
                span { class: "font-semibold text-star-white", "{piece.name}" }
                span { class: "{kind_class}", "{piece.kind}" }
                span { class: "text-stardust text-sm", "{n} sizes" }
                span { class: "text-stardust text-sm catalog-range", "{range}" }
            }
            div { class: "overflow-x-auto",
                table { class: "table-cosmic table-orders",
                    thead {
                        tr {
                            th { "Size" }
                            th { title: "Volume (cm\u{00b3})", "Vol" }
                            th { title: "Silver weight (g)", "Ag g" }
                            th { title: "Silver cost", "Ag $" }
                            th { title: "14K gold cost", "Au $" }
                            th { title: "Bronze cost", "Bz $" }
                            th { title: "Wax cost", "Wax $" }
                        }
                    }
                    tbody {
                        for s in sizes.iter() {
                            CatalogRow { size: s.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn CatalogRow(size: PieceCostSize) -> Element {
    let fmt = |v: Option<f64>| v.map(|x| format!("{:.2}", x)).unwrap_or_else(|| "\u{2014}".to_string());
    let money = |v: Option<f64>| v.map(|x| format!("$ {:.2}", x)).unwrap_or_else(|| "\u{2014}".to_string());
    let ring = size.ring_size.clone().unwrap_or_default();
    let (vol, ag_g, ag, au, bz, wax) = (
        fmt(size.volume_cm3),
        fmt(size.silver_g),
        money(size.silver_usd),
        money(size.gold_usd),
        money(size.bronze_usd),
        money(size.wax_usd),
    );
    rsx! {
        tr {
            td { class: "td-nowrap font-mono text-aurora-purple", "{ring}" }
            td { class: "td-nowrap text-stardust", "{vol}" }
            td { class: "td-nowrap text-stardust", "{ag_g}" }
            td { class: "td-nowrap text-star-white", "{ag}" }
            td { class: "td-nowrap text-stardust", "{au}" }
            td { class: "td-nowrap text-stardust", "{bz}" }
            td { class: "td-nowrap text-stardust", "{wax}" }
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
    catalog: Vec<CatalogPiece>,
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
        OrderSource::Shopify => ("Shopify", "badge-method"),
        OrderSource::Etsy => ("Etsy", "badge-nebula"),
        OrderSource::Custom => ("Custom", "badge-success"),
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
        let cw = lookup_piece_cost(item, &catalog);
        let q = item.quantity as f64;
        (
            c + cw.as_ref().map(|x| x.cost_usd * q).unwrap_or(0.0),
            w + cw.as_ref().map(|x| x.weight_g * q).unwrap_or(0.0),
        )
    });
    let cost_str = if order_cost > 0.0 {
        format!("$ {:.2}", order_cost)
    } else {
        "\u{2014}".to_string()
    };
    let weight_str = if order_weight > 0.0 {
        format!("{:.1} g", order_weight)
    } else {
        "\u{2014}".to_string()
    };

    let (margin_str, margin_class) = if order_cost > 0.0 && order.total_price > 0.0 {
        let margin = order.total_price - order_cost;
        let pct = margin / order.total_price * 100.0;
        let color = if margin < 0.0 { "text-warning-red" } else { "text-alien-green" };
        (format!("$ {:.2} ({:.0}%)", margin, pct), format!("td-nowrap font-semibold {color}"))
    } else {
        ("\u{2014}".to_string(), "td-nowrap text-stardust".to_string())
    };

    rsx! {
        tr {
            class: "{urgency_class} order-row-clickable",
            onclick: move |evt| on_click.call(evt),
            td { class: "td-thumb",
                {match first_image.as_deref() {
                    Some(url) => rsx! { img { class: "order-thumb", src: "{url}", alt: "" } },
                    None => rsx! { span { class: "order-thumb-placeholder", "pkg" } },
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
            td { class: "{margin_class}", title: "Sale price minus our cost", "{margin_str}" }
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
    catalog: Vec<CatalogPiece>,
    on_close: EventHandler<MouseEvent>,
    on_set_state: EventHandler<(String, bool, bool)>,
    on_set_charge: EventHandler<(String, f64)>,
) -> Element {
    let source_label = match order.source {
        OrderSource::Shopify => "Shopify",
        OrderSource::Etsy => "Etsy",
        OrderSource::Custom => "Custom",
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
    let archived = order.archived;
    let completed = order.completed;
    let complete_label = if completed { "Reopen" } else { "Complete" };
    let archive_label = if archived { "Unarchive" } else { "Archive" };
    let key_complete = order.state_key();
    let key_archive = order.state_key();
    let is_custom = order.source == OrderSource::Custom;
    let order_id = order.id.clone();
    let mut charge_input = use_signal(|| {
        if order.total_price > 0.0 {
            format!("{}", order.total_price)
        } else {
            String::new()
        }
    });

    let order_cost: f64 = order
        .items
        .iter()
        .map(|item| {
            (item.quantity as f64)
                * lookup_piece_cost(item, &catalog).as_ref().map(|x| x.cost_usd).unwrap_or(0.0)
        })
        .sum();
    let cost_block = (order_cost > 0.0).then(|| {
        let s = format!("$ {:.2}", order_cost);
        let margin = order.total_price - order_cost;
        let pct = if order.total_price > 0.0 { margin / order.total_price * 100.0 } else { 0.0 };
        let margin_s = format!("$ {:.2} ({:.0}%)", margin, pct);
        let margin_color = if margin < 0.0 {
            "font-semibold text-warning-red"
        } else {
            "font-semibold text-alien-green"
        };
        (s, margin_s, margin_color)
    });

    rsx! {
        div { class: "flex items-center justify-between mb-4 flex-wrap gap-2 flex-shrink-0",
            h2 { class: "text-xl font-bold text-star-white",
                "{order.order_number}"
            }
            div { class: "flex items-center gap-2 flex-wrap",
                span { class: "badge badge-nebula", "{source_label}" }
                button {
                    class: "btn-cosmic text-sm",
                    onclick: move |_| on_set_state.call((key_complete.clone(), archived, !completed)),
                    "{complete_label}"
                }
                button {
                    class: "btn-cosmic text-sm",
                    onclick: move |_| on_set_state.call((key_archive.clone(), !archived, completed)),
                    "{archive_label}"
                }
                button {
                    class: "btn-cosmic text-sm",
                    onclick: move |evt| on_close.call(evt),
                    "Close"
                }
            }
        }
        div { class: "od-body flex-1 overflow-y-auto min-h-0",
            {match order.source {
                OrderSource::Etsy => rsx! {
                    p { class: "text-stardust text-sm mb-3",
                        "Receipt ID: {order.id}"
                    }
                },
                OrderSource::Shopify => rsx! { },
                OrderSource::Custom => rsx! { },
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
                {cost_block.as_ref().map(|(s, margin_s, margin_color)| rsx! {
                    dt { "Our cost" }
                    dd { class: "font-semibold text-aurora-purple", "{s}" }
                    dt { "Margin" }
                    dd { class: "{margin_color}", "{margin_s}" }
                })}
            }
            {is_custom.then(|| rsx! {
                div { class: "mt-4",
                    p { class: "text-stardust text-sm font-medium mb-2", "Charge (what you're billing)" }
                    div { class: "charge-edit",
                        span { class: "text-stardust", "$" }
                        input {
                            r#type: "number", min: "0", step: "0.01", placeholder: "0.00",
                            value: "{charge_input}",
                            oninput: move |e| charge_input.set(e.value())
                        }
                        button {
                            class: "btn-nebula", r#type: "button",
                            onclick: move |_| {
                                let v: f64 = charge_input.read().trim().parse().unwrap_or(0.0);
                                on_set_charge.call((order_id.clone(), v));
                            },
                            "Save charge"
                        }
                    }
                }
            })}
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
                            cost_weight: lookup_piece_cost(item, &catalog),
                        }
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
        None => ("\u{2014}".to_string(), "\u{2014}".to_string()),
    };
    let margin_str = match &cost_weight {
        Some(cw) => {
            let qty = item.quantity as f64;
            format!("${:.2}", item.price * qty - cw.cost_usd * qty)
        }
        None => "\u{2014}".to_string(),
    };
    rsx! {
        div { class: "flex items-start gap-3 p-3 rounded-lg bg-nebula-dark/50 border border-nebula-purple/20",
            {item.image_url.as_ref().map(|url| rsx! {
                img { class: "w-14 h-14 rounded object-cover flex-shrink-0", src: "{url}", alt: "" }
            }).unwrap_or(rsx! {
                div { class: "w-14 h-14 rounded bg-nebula-purple/20 flex items-center justify-center flex-shrink-0 text-2xl", "pkg" }
            })}
            div { class: "min-w-0 flex-1",
                p { class: "font-medium text-star-white", "{item.name}" }
                {(item.quantity > 1).then(|| rsx! { p { class: "text-stardust text-sm", "Qty: {item.quantity}" } })}
                {item.variant_info.as_ref().map(|v| rsx! { p { class: "text-stardust text-sm", "{v}" } })}
                {item.ring_size.as_ref().map(|s| rsx! { p { class: "text-aurora-purple text-sm font-mono", "Size: {s}" } })}
                p { class: "text-moonlight text-sm", "{item.metal_type.display_name()} | {price_str}" }
                p { class: "text-stardust text-sm mt-1",
                    "Our cost: {cost_str} | Margin: {margin_str} | Weight: {weight_str}"
                }
            }
        }
    }
}

#[component]
fn DraftLineRow(index: usize, line: DraftLine, lines: Signal<Vec<DraftLine>>) -> Element {
    rsx! {
        div { class: "draft-line",
            div { class: "min-w-0",
                div { class: "draft-line-name",
                    {line.image_url.as_ref().map(|url| rsx! {
                        img { class: "order-thumb", src: "{url}", alt: "" }
                    })}
                    {if line.catalog {
                        rsx! { span { "{line.name}" } }
                    } else {
                        rsx! {
                            input {
                                r#type: "text", class: "dl-name", placeholder: "Item name",
                                value: "{line.name}",
                                oninput: move |e| { lines.write()[index].name = e.value(); }
                            }
                        }
                    }}
                }
                div { class: "draft-line-fields mt-2",
                    {if !line.sizes.is_empty() {
                        rsx! {
                            select {
                                class: "dl-size",
                                onchange: move |e| { lines.write()[index].size = e.value(); },
                                option { value: "", "No size" }
                                for s in line.sizes.iter() {
                                    option { value: "{s}", selected: *s == line.size, "{s}" }
                                }
                            }
                        }
                    } else {
                        rsx! {
                            input {
                                r#type: "text", class: "dl-size", placeholder: "Size",
                                value: "{line.size}",
                                oninput: move |e| { lines.write()[index].size = e.value(); }
                            }
                        }
                    }}
                    select {
                        class: "dl-metal",
                        onchange: move |e| { lines.write()[index].metal = e.value(); },
                        option { value: "Silver", selected: line.metal == "Silver", "Silver" }
                        option { value: "Gold Plated", selected: line.metal == "Gold Plated", "Gold Plated" }
                        option { value: "Bronze", selected: line.metal == "Bronze", "Bronze" }
                    }
                    input {
                        r#type: "number", class: "dl-qty", min: "1", placeholder: "Qty",
                        value: "{line.qty}",
                        oninput: move |e| {
                            let q: u32 = e.value().trim().parse().unwrap_or(1).max(1);
                            lines.write()[index].qty = q;
                        }
                    }
                    input {
                        r#type: "number", class: "dl-price", min: "0", step: "0.01", placeholder: "$ each",
                        value: "{line.price}",
                        oninput: move |e| { lines.write()[index].price = e.value(); }
                    }
                }
            }
            button {
                class: "draft-line-remove", r#type: "button",
                onclick: move |_| {
                    let mut v = lines.write();
                    if index < v.len() { v.remove(index); }
                },
                "\u{00d7}"
            }
        }
    }
}
