//! SurrealDB connection singleton (server-only).
//! Set SURREAL_URL in env (e.g. ws://127.0.0.1:8000) and call ensure_db_init() before querying.

use std::sync::LazyLock;
use surrealdb::engine::remote::ws::{Client, Ws, Wss};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;
use surrealdb_types::SurrealValue;

const NS: &str = "jewelry_calculator";
const DB_NAME: &str = "jewelry_calculator";

/// Singleton DB; connect with ensure_db_init() at startup when SURREAL_URL is set.
pub static DB: LazyLock<Surreal<Client>> = LazyLock::new(Surreal::init);

static DB_INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Connect the singleton DB exactly once. Safe to call repeatedly (subsequent calls are no-ops).
pub async fn ensure_db_init() -> Result<(), String> {
    DB_INIT
        .get_or_try_init(|| async {
            let url = std::env::var("SURREAL_URL")
                .map_err(|_| "SURREAL_URL not set".to_string())?;
            let url = url.trim().to_string();
            if url.is_empty() {
                return Err("SURREAL_URL is empty".to_string());
            }
            // Typed Ws/Wss engines prepend the scheme; pass host only, not the full URL.
            let connect_result = if let Some(host) = url.strip_prefix("wss://") {
                DB.connect::<Wss>(host).await
            } else if let Some(host) = url.strip_prefix("ws://") {
                DB.connect::<Ws>(host).await
            } else {
                DB.connect::<Ws>(url.as_str()).await
            };
            match &connect_result {
                Ok(_) => eprintln!("Connected to SurrealDB at {}", url),
                Err(e) => eprintln!("Failed connecting to {}: {:?}", url, e),
            }
            connect_result.map_err(|e| e.to_string())?;
            if let (Ok(user), Ok(pass)) =
                (std::env::var("SURREAL_USER"), std::env::var("SURREAL_PASS"))
            {
                DB.signin(Root { username: user.clone(), password: pass })
                    .await
                    .map_err(|e| e.to_string())?;
                eprintln!("Signed in to SurrealDB as {}", user);
            }
            DB.use_ns(NS).use_db(DB_NAME).await.map_err(|e| e.to_string())?;
            eprintln!("Using NS: {}, DB: {}", NS, DB_NAME);
            if let Err(e) = migrate_orders().await {
                crate::log::app_log("ERROR", format!("Order archive migration failed: {}", e));
            }
            Ok(())
        })
        .await
        .map(|_| ())
}

use crate::model::{MetalType, Order, OrderItem, OrderSource};
use surrealdb_types::File;

fn metal_tag(m: &MetalType) -> &'static str {
    match m {
        MetalType::Gold => "gold",
        MetalType::SolidGold => "goldsolid",
        MetalType::Silver => "silver",
        MetalType::Bronze => "bronze",
        MetalType::Unknown => "unknown",
    }
}

fn source_tag(s: &OrderSource) -> &'static str {
    match s {
        OrderSource::Shopify => "shopify",
        OrderSource::Etsy => "etsy",
        OrderSource::Custom => "custom",
    }
}

/// An order item as stored in SurrealDB: image is a native `record<file>`
/// pointer into the thumbnails bucket, not a string.
#[derive(SurrealValue, Clone)]
struct CachedItem {
    name: String,
    quantity: i64,
    price: f64,
    metal: String,
    ring_size: Option<String>,
    variant_info: Option<String>,
    image: Option<File>,
}

impl CachedItem {
    fn from_item(it: &OrderItem) -> Self {
        // image_url is "thumb/<key>" once cached; turn it into a file pointer.
        let image = it
            .image_url
            .as_deref()
            .and_then(|u| u.strip_prefix("thumb/"))
            .map(|key| File::new(THUMB_BUCKET, key));
        CachedItem {
            name: it.name.clone(),
            quantity: it.quantity as i64,
            price: it.price,
            metal: metal_tag(&it.metal_type).to_string(),
            ring_size: it.ring_size.clone(),
            variant_info: it.variant_info.clone(),
            image,
        }
    }

    fn into_item(self) -> OrderItem {
        let image_url = self
            .image
            .map(|f| format!("thumb/{}", f.key().trim_start_matches('/')));
        OrderItem {
            name: self.name,
            quantity: self.quantity.max(0) as u32,
            price: self.price,
            metal_type: MetalType::from_string(&self.metal),
            ring_size: self.ring_size,
            variant_info: self.variant_info,
            image_url,
        }
    }
}

/// An order stored as a real SurrealDB object (the `payload`).
#[derive(SurrealValue, Clone)]
struct CachedOrder {
    order_id: String,
    source: String,
    order_number: String,
    customer_name: String,
    items: Vec<CachedItem>,
    order_date: chrono::DateTime<chrono::Utc>,
    due_date: chrono::DateTime<chrono::Utc>,
    total_price: f64,
    currency: String,
    status: String,
    shipping_address: Option<String>,
    /// Fulfilled/shipped/cancelled at the source. Option tolerates rows written
    /// before this field existed. The order_state overlay still outranks it.
    completed: Option<bool>,
}

impl CachedOrder {
    fn from_order(o: &Order) -> Self {
        CachedOrder {
            order_id: o.id.clone(),
            source: source_tag(&o.source).to_string(),
            order_number: o.order_number.clone(),
            customer_name: o.customer_name.clone(),
            items: o.items.iter().map(CachedItem::from_item).collect(),
            order_date: o.order_date,
            due_date: o.due_date,
            total_price: o.total_price,
            currency: o.currency.clone(),
            status: o.status.clone(),
            shipping_address: o.shipping_address.clone(),
            completed: Some(o.completed),
        }
    }

    fn into_order(self) -> Order {
        Order {
            id: self.order_id,
            source: match self.source.as_str() {
                "etsy" => OrderSource::Etsy,
                "custom" => OrderSource::Custom,
                _ => OrderSource::Shopify,
            },
            order_number: self.order_number,
            customer_name: self.customer_name,
            items: self.items.into_iter().map(CachedItem::into_item).collect(),
            order_date: self.order_date,
            due_date: self.due_date,
            total_price: self.total_price,
            currency: self.currency,
            status: self.status,
            shipping_address: self.shipping_address,
            archived: false,
            completed: self.completed.unwrap_or(false),
            notes: None,
            stage: None,
        }
    }
}

/// One row of the persistent order archive. `key` is bound as the record id, not
/// written as a column: the `orders` table is SCHEMAFULL and declares only
/// source, order_number, payload and fetched_at.
#[derive(SurrealValue, Clone)]
struct OrderCacheRow {
    key: String,
    source: String,
    order_number: String,
    payload: CachedOrder,
}

#[derive(SurrealValue)]
struct CountRow {
    n: i64,
}

/// Record-id keys must be non-empty and free of control characters.
fn valid_record_key(key: &str) -> bool {
    !key.is_empty() && !key.chars().any(char::is_control)
}

/// Rows per statement; surrealkv takes one writer, so keep each batch bounded.
const ORDER_CHUNK: usize = 250;

/// Add or update orders keyed by Order::state_key. Never deletes: the archive is
/// the source of truth for the order set, the APIs only supply updates.
pub async fn upsert_orders(orders: &[Order]) -> Result<(), String> {
    let rows: Vec<OrderCacheRow> = orders
        .iter()
        .filter(|o| valid_record_key(&o.state_key()))
        .map(|o| OrderCacheRow {
            key: o.state_key(),
            source: source_tag(&o.source).to_string(),
            order_number: o.order_number.clone(),
            payload: CachedOrder::from_order(o),
        })
        .collect();
    let skipped = orders.len() - rows.len();
    if skipped > 0 {
        crate::log::app_log(
            "ERROR",
            format!("Order archive: skipped {} order(s) with an unusable record key.", skipped),
        );
    }
    for chunk in rows.chunks(ORDER_CHUNK) {
        DB.query(
            "FOR $r IN $rows { \
                 UPSERT type::record('orders', $r.key) SET \
                     source = $r.source, \
                     order_number = $r.order_number, \
                     payload = $r.payload; \
             };",
        )
        .bind(("rows", chunk.to_vec()))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The order_state rows that decide open vs past, split by what they decide.
struct StateSplit {
    /// Explicitly archived or completed by the user.
    closed: Vec<String>,
    /// Explicitly re-opened, which outranks a source-completed payload.
    reopened: Vec<String>,
    /// Explicitly archived.
    archived: Vec<String>,
}

async fn state_split() -> Result<StateSplit, String> {
    let mut res = DB
        .query(
            "SELECT VALUE record::id(id) FROM order_state WHERE archived = true OR completed = true; \
             SELECT VALUE record::id(id) FROM order_state WHERE completed = false AND archived != true; \
             SELECT VALUE record::id(id) FROM order_state WHERE archived = true;",
        )
        .await
        .map_err(|e| e.to_string())?;
    let closed: Vec<String> = res.take(0).map_err(|e| e.to_string())?;
    let reopened: Vec<String> = res.take(1).map_err(|e| e.to_string())?;
    let archived: Vec<String> = res.take(2).map_err(|e| e.to_string())?;
    Ok(StateSplit { closed, reopened, archived })
}

/// An order is past when the user closed it, or the source reports it
/// fulfilled/shipped/cancelled and the user has not explicitly re-opened it.
const PAST_PRED: &str = "(record::id(id) IN $closed \
    OR (payload.completed = true AND record::id(id) NOT IN $reopened))";

/// Open work only. Past orders are dropped at the DB level so the board stays
/// bounded as the archive grows. Never capped.
pub async fn load_open_orders() -> Result<Vec<Order>, String> {
    let s = state_split().await?;
    let mut res = DB
        .query(format!(
            "SELECT VALUE payload FROM orders WHERE !{PAST_PRED} \
             ORDER BY payload.order_date DESC"
        ))
        .bind(("closed", s.closed))
        .bind(("reopened", s.reopened))
        .await
        .map_err(|e| e.to_string())?;
    let cached: Vec<CachedOrder> = res.take(0).map_err(|e| e.to_string())?;
    Ok(cached.into_iter().map(CachedOrder::into_order).collect())
}

/// Newest fetched_at in the archive; None when empty. Freshness only decides
/// whether to ALSO refresh — never whether to return rows.
pub async fn cache_age() -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let mut res = DB
        .query("SELECT VALUE fetched_at FROM orders ORDER BY fetched_at DESC LIMIT 1")
        .await
        .map_err(|e| e.to_string())?;
    let rows: Vec<chrono::DateTime<chrono::Utc>> = res.take(0).map_err(|e| e.to_string())?;
    Ok(rows.into_iter().next())
}

/// Matches an already-lowercased $q against order number, customer and item names.
const HISTORY_SEARCH: &str = "string::contains(string::lowercase(string::concat(\
    order_number, ' ', (payload.customer_name ?? ''), ' ', \
    array::join((payload.items.name ?? []), ' '))), $q)";

/// The Rust twin of [HISTORY_SEARCH], for orders not stored in the archive.
fn order_text_matches(o: &Order, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let mut hay = format!("{} {}", o.order_number, o.customer_name);
    for it in &o.items {
        hay.push(' ');
        hay.push_str(&it.name);
    }
    hay.to_lowercase().contains(q)
}

/// Past custom orders matching the same filters as the archive query. They live
/// in custom_orders, not orders, so History must fold them in explicitly.
async fn past_custom_orders(
    since: Option<chrono::DateTime<chrono::Utc>>,
    q: &str,
    src: &str,
    state: &str,
) -> Vec<Order> {
    if !src.is_empty() && src != "custom" {
        return Vec::new();
    }
    let mut custom = match load_custom_orders().await {
        Ok(v) => v,
        Err(e) => {
            crate::log::app_log("ERROR", format!("History: custom orders unavailable: {}", e));
            return Vec::new();
        }
    };
    apply_order_state(&mut custom).await;
    custom.retain(|o| {
        (o.archived || o.completed)
            && since.map_or(true, |s| o.order_date > s)
            && match state {
                "archived" => o.archived,
                "completed" => !o.archived,
                _ => true,
            }
            && order_text_matches(o, q)
    });
    custom
}

/// One page of past orders (newest first) plus the total matching count.
/// `days` = 0 means all time; `source` is "shopify" | "etsy" | "custom", None =
/// all; `state` is "archived" | "completed", empty = every past order.
pub async fn load_order_history(
    days: i64,
    query: &str,
    source: Option<&str>,
    state: &str,
    limit: usize,
    offset: usize,
) -> Result<(Vec<Order>, usize), String> {
    let since: Option<chrono::DateTime<chrono::Utc>> = if days > 0 {
        Some(chrono::Utc::now() - chrono::Duration::days(days))
    } else {
        None
    };
    let q = query.trim().to_lowercase();
    let src = source.unwrap_or("").trim().to_string();
    let s = state_split().await?;
    // Past-only, so an open order can never be reachable from History alone.
    let mut preds: Vec<&str> = vec![PAST_PRED];
    match state {
        "archived" => preds.push("record::id(id) IN $archived"),
        "completed" => preds.push("record::id(id) NOT IN $archived"),
        _ => {}
    }
    if since.is_some() {
        preds.push("payload.order_date > $since");
    }
    if !src.is_empty() {
        preds.push("source = $src");
    }
    if !q.is_empty() {
        preds.push(HISTORY_SEARCH);
    }
    let filter = format!("WHERE {}", preds.join(" AND "));
    // Take the first offset+limit archive rows rather than the exact page: custom
    // orders are merged in below, and no row past that index can surface inside it.
    let take = offset.saturating_add(limit) as i64;
    let sql = format!(
        "SELECT VALUE payload FROM orders {filter} \
         ORDER BY payload.order_date DESC LIMIT $take; \
         SELECT count() AS n FROM orders {filter} GROUP ALL;"
    );
    let mut res = DB
        .query(sql)
        .bind(("since", since))
        .bind(("src", src.clone()))
        .bind(("q", q.clone()))
        .bind(("closed", s.closed))
        .bind(("reopened", s.reopened))
        .bind(("archived", s.archived))
        .bind(("take", take))
        .await
        .map_err(|e| e.to_string())?;
    let cached: Vec<CachedOrder> = res.take(0).map_err(|e| e.to_string())?;
    let counts: Vec<CountRow> = res.take(1).map_err(|e| e.to_string())?;
    let total = counts.first().map(|c| c.n.max(0) as usize).unwrap_or(0);

    let custom = past_custom_orders(since, &q, &src, state).await;
    let mut rows: Vec<Order> = cached.into_iter().map(CachedOrder::into_order).collect();
    let total = total + custom.len();
    rows.extend(custom);
    rows.sort_by(|a, b| b.order_date.cmp(&a.order_date));
    let page = rows.into_iter().skip(offset).take(limit).collect();
    Ok((page, total))
}

/// Archive rows written before the record id became Order::state_key.
const UNKEYED_ROWS: &str = "WHERE !string::starts_with(<string>record::id(id), 'shopify_') \
    AND !string::starts_with(<string>record::id(id), 'etsy_') \
    AND !string::starts_with(<string>record::id(id), 'custom_')";

/// Drop archive rows with a random record id, which would otherwise shadow the
/// keyed rows forever. Idempotent: after the first run the DELETE matches nothing.
async fn migrate_orders() -> Result<(), String> {
    let mut res = DB
        .query(format!("SELECT count() AS n FROM orders {UNKEYED_ROWS} GROUP ALL"))
        .await
        .map_err(|e| e.to_string())?;
    let counts: Vec<CountRow> = res.take(0).map_err(|e| e.to_string())?;
    let n = counts.first().map(|c| c.n.max(0)).unwrap_or(0);
    if n == 0 {
        return Ok(());
    }
    DB.query(format!("DELETE orders {UNKEYED_ROWS}"))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    crate::log::app_log(
        "ERROR",
        format!("Order archive migration: deleted {} unkeyed row(s).", n),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Order state overlay + custom orders
// ---------------------------------------------------------------------------

#[derive(SurrealValue, Clone)]
struct SizeOverride {
    idx: i64,
    size: String,
}

#[derive(SurrealValue)]
struct OrderStateRow {
    rid: String,
    archived: Option<bool>,
    completed: Option<bool>,
    notes: Option<String>,
    stage: Option<String>,
    size_overrides: Option<Vec<SizeOverride>>,
}

/// Per-order overlay persisted in order_state (keyed by Order::state_key).
/// `archived` and `completed` stay None on a row that only carries notes, a
/// stage or size overrides, so those rows never reset the order's flags.
#[derive(Clone, Default)]
pub struct OrderStateData {
    pub archived: Option<bool>,
    pub completed: Option<bool>,
    pub notes: Option<String>,
    pub stage: Option<String>,
    /// item index -> overridden ring size.
    pub size_overrides: std::collections::HashMap<usize, String>,
}

/// Map of state_key -> overlay from the persistent order_state table.
pub async fn load_order_state() -> Result<std::collections::HashMap<String, OrderStateData>, String> {
    let mut res = DB
        .query("SELECT <string>id AS rid, archived, completed, notes, stage, size_overrides FROM order_state")
        .await
        .map_err(|e| e.to_string())?;
    let rows: Vec<OrderStateRow> = res.take(0).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let key = r.rid.strip_prefix("order_state:").unwrap_or(&r.rid).to_string();
            let size_overrides = r
                .size_overrides
                .unwrap_or_default()
                .into_iter()
                .filter(|o| o.idx >= 0)
                .map(|o| (o.idx as usize, o.size))
                .collect();
            (
                key,
                OrderStateData {
                    archived: r.archived,
                    completed: r.completed,
                    notes: r.notes,
                    stage: r.stage,
                    size_overrides,
                },
            )
        })
        .collect())
}

/// Set archive/complete flags for an order key (e.g. "shopify_123", "custom_abc").
pub async fn set_order_state(key: &str, archived: bool, completed: bool) -> Result<(), String> {
    DB.query("UPSERT type::record('order_state', $key) SET archived = $a, completed = $c")
        .bind(("key", key.to_string()))
        .bind(("a", archived))
        .bind(("c", completed))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Set the free-text production notes for an order key (empty clears).
pub async fn set_order_notes(key: &str, notes: &str) -> Result<(), String> {
    let value: Option<String> = if notes.trim().is_empty() { None } else { Some(notes.to_string()) };
    DB.query("UPSERT type::record('order_state', $key) SET notes = $n")
        .bind(("key", key.to_string()))
        .bind(("n", value))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Set the production stage for an order key (empty clears).
pub async fn set_order_stage(key: &str, stage: &str) -> Result<(), String> {
    let value: Option<String> = if stage.trim().is_empty() { None } else { Some(stage.to_string()) };
    DB.query("UPSERT type::record('order_state', $key) SET stage = $s")
        .bind(("key", key.to_string()))
        .bind(("s", value))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Override (or clear, when `size` is empty) the ring size of item `idx` in an
/// order. Stored on order_state so it survives the cache and re-prices from the
/// catalog for the new size.
pub async fn set_size_override(key: &str, idx: i64, size: &str) -> Result<(), String> {
    if size.trim().is_empty() {
        DB.query("UPSERT type::record('order_state', $key) SET size_overrides = (size_overrides ?? []).filter(|$o| $o.idx != $idx)")
            .bind(("key", key.to_string()))
            .bind(("idx", idx))
            .await
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
    } else {
        DB.query("UPSERT type::record('order_state', $key) SET size_overrides = (size_overrides ?? []).filter(|$o| $o.idx != $idx) + [{ idx: $idx, size: $size }]")
            .bind(("key", key.to_string()))
            .bind(("idx", idx))
            .bind(("size", size.to_string()))
            .await
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// State keys currently sitting at a given production stage.
pub async fn keys_in_stage(stage: &str) -> Result<Vec<String>, String> {
    let mut res = DB
        .query("SELECT VALUE <string>id FROM order_state WHERE stage = $s")
        .bind(("s", stage.to_string()))
        .await
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = res.take(0).map_err(|e| e.to_string())?;
    Ok(ids
        .into_iter()
        .map(|i| i.strip_prefix("order_state:").unwrap_or(&i).to_string())
        .collect())
}

/// Advance every order at `from` stage to `to`. Returns how many moved.
pub async fn advance_stage(from: &str, to: &str) -> Result<usize, String> {
    let keys = keys_in_stage(from).await?;
    if keys.is_empty() {
        return Ok(0);
    }
    DB.query("UPDATE order_state SET stage = $to WHERE stage = $from")
        .bind(("from", from.to_string()))
        .bind(("to", to.to_string()))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(keys.len())
}

#[derive(SurrealValue)]
struct CustomRow {
    rid: String,
    payload: CachedOrder,
}

/// All custom orders, each with id = the custom_orders record key and source = Custom.
pub async fn load_custom_orders() -> Result<Vec<Order>, String> {
    let mut res = DB
        .query("SELECT <string>id AS rid, payload FROM custom_orders")
        .await
        .map_err(|e| e.to_string())?;
    let rows: Vec<CustomRow> = res.take(0).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut o = r.payload.into_order();
            o.source = OrderSource::Custom;
            o.id = r.rid.strip_prefix("custom_orders:").unwrap_or(&r.rid).to_string();
            o
        })
        .collect())
}

/// Persist a new custom order.
pub async fn save_custom_order(order: &Order) -> Result<(), String> {
    let payload = CachedOrder::from_order(order);
    DB.query("CREATE custom_orders SET payload = $payload")
        .bind(("payload", payload))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Replace an existing custom order's payload (id = custom_orders record key).
pub async fn update_custom_order(order: &Order) -> Result<(), String> {
    let payload = CachedOrder::from_order(order);
    DB.query("UPDATE type::record('custom_orders', $key) SET payload = $payload")
        .bind(("key", order.id.clone()))
        .bind(("payload", payload))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Apply the archive/complete/notes/stage/size overlay from order_state.
pub async fn apply_order_state(orders: &mut [Order]) {
    let state = match load_order_state().await {
        Ok(s) => s,
        Err(e) => {
            crate::log::app_log("ERROR", format!("Order state overlay: {}", e));
            return;
        }
    };
    for o in orders.iter_mut() {
        if let Some(s) = state.get(&o.state_key()) {
            if let Some(archived) = s.archived {
                o.archived = archived;
            }
            if let Some(completed) = s.completed {
                o.completed = completed;
            }
            o.notes = s.notes.clone();
            o.stage = s.stage.clone();
            for (i, item) in o.items.iter_mut().enumerate() {
                if let Some(sz) = s.size_overrides.get(&i) {
                    item.ring_size = Some(sz.clone());
                }
            }
        }
    }
}

/// Merge live/cached Shopify+Etsy orders with custom orders, then apply the
/// archive/complete overlay from order_state.
pub async fn merge_orders(mut base: Vec<Order>) -> Vec<Order> {
    match load_custom_orders().await {
        Ok(custom) => base.extend(custom),
        Err(e) => crate::log::app_log("ERROR", format!("Custom orders: {}", e)),
    }
    apply_order_state(&mut base).await;
    base
}

/// Order keys already announced to Home Assistant.
pub async fn load_seen_keys() -> Result<std::collections::HashSet<String>, String> {
    let mut res = DB
        .query("SELECT VALUE <string>id FROM seen_orders")
        .await
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = res.take(0).map_err(|e| e.to_string())?;
    Ok(ids
        .into_iter()
        .map(|i| i.strip_prefix("seen_orders:").unwrap_or(&i).to_string())
        .collect())
}

/// Mark order keys as announced.
pub async fn mark_seen(keys: &[String]) -> Result<(), String> {
    for key in keys {
        DB.query("UPSERT type::record('seen_orders', $key) SET seen_at = time::now()")
            .bind(("key", key.clone()))
            .await
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Thumbnail bucket (cached Shopify/Etsy product images)
// ---------------------------------------------------------------------------

const THUMB_BUCKET: &str = "thumbnails";

fn thumb_content_type(key: &str) -> &'static str {
    if key.ends_with(".png") {
        "image/png"
    } else if key.ends_with(".webp") {
        "image/webp"
    } else if key.ends_with(".gif") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

/// Keys are our own `<hash>.<ext>` / `render<slug>.png` slugs; reject anything
/// else (the key is interpolated into the file literal).
fn safe_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

async fn thumb_exists(key: &str) -> bool {
    let q = format!("RETURN file::exists(f\"{THUMB_BUCKET}:/{key}\")");
    if let Ok(mut res) = DB.query(q).await {
        if let Ok(Some(b)) = res.take::<Option<bool>>(0) {
            return b;
        }
    }
    false
}

/// Store image bytes in the thumbnails bucket (base64 in, decoded server-side).
pub async fn put_thumbnail(key: &str, bytes: &[u8]) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    if !safe_key(key) {
        return Err("invalid thumbnail key".into());
    }
    let b64 = STANDARD.encode(bytes);
    let q = format!("f\"{THUMB_BUCKET}:/{key}\".put(encoding::base64::decode($b64))");
    DB.query(q).bind(("b64", b64)).await.map_err(|e| e.to_string())?.check().map_err(|e| e.to_string())?;
    Ok(())
}

/// Fetch a thumbnail's bytes + content-type from the bucket.
pub async fn get_thumbnail(key: &str) -> Option<(Vec<u8>, String)> {
    use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
    if !safe_key(key) {
        return None;
    }
    ensure_db_init().await.ok()?;
    let q = format!("RETURN encoding::base64::encode(f\"{THUMB_BUCKET}:/{key}\".get())");
    let mut res = DB.query(q).await.ok()?;
    let b64: Option<String> = res.take(0).ok()?;
    let bytes = STANDARD_NO_PAD.decode(b64?.trim_end_matches('=')).ok()?;
    Some((bytes, thumb_content_type(key).to_string()))
}

/// Download an image once, cache it in the bucket, and return the app-relative
/// `thumb/<key>` URL OrderTracker serves it under.
pub async fn cache_thumbnail(client: &reqwest::Client, url: &str) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ext = if ct.contains("png") {
        "png"
    } else if ct.contains("webp") {
        "webp"
    } else if ct.contains("gif") {
        "gif"
    } else {
        "jpg"
    };
    let key = format!("{:016x}.{}", h.finish(), ext);
    if !thumb_exists(&key).await {
        let bytes = resp.bytes().await.ok()?;
        put_thumbnail(&key, &bytes).await.ok()?;
    }
    Some(format!("thumb/{}", key))
}

/// Add a product key (an Etsy/Shopify item name) to a catalog jewelry piece so
/// its orders resolve to that piece's costs. Matches the piece by display name.
pub async fn link_product(piece_name: &str, product_key: &str) -> Result<(), String> {
    DB.query("UPDATE jewelry SET product_keys = array::union(product_keys ?? [], [$key]) WHERE name = $name")
        .bind(("name", piece_name.to_string()))
        .bind(("key", product_key.to_string()))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Link a Shopify/Etsy listing to a catalog piece: add the listing title to
/// product_keys, cache its image as the piece thumbnail, and record sale price.
pub async fn link_listing(
    piece_name: &str,
    product_key: &str,
    image_url: Option<&str>,
    sale_price: Option<f64>,
) -> Result<(), String> {
    link_product(piece_name, product_key).await?;

    let thumb_file = match image_url {
        Some(url) if url.starts_with("http") => {
            let client = reqwest::Client::new();
            cache_thumbnail(&client, url)
                .await
                .and_then(|p| p.strip_prefix("thumb/").map(|k| File::new(THUMB_BUCKET, k)))
        }
        _ => None,
    };

    if let Some(file) = thumb_file {
        DB.query("UPDATE jewelry SET thumbnail = $t, sale_price = $p ?? sale_price WHERE name = $name")
            .bind(("name", piece_name.to_string()))
            .bind(("t", file))
            .bind(("p", sale_price))
            .await
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
    } else if sale_price.is_some() {
        DB.query("UPDATE jewelry SET sale_price = $p WHERE name = $name")
            .bind(("name", piece_name.to_string()))
            .bind(("p", sale_price))
            .await
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Server-side catalog row: thumbnail is a native file pointer, mapped to the
/// served "thumb/<key>" path for [CatalogPiece].
#[derive(SurrealValue)]
struct CatalogRowDb {
    name: String,
    kind: String,
    product_keys: Option<Vec<String>>,
    thumbnail: Option<File>,
    render: Option<File>,
    sale_price: Option<f64>,
    sizes: Vec<crate::model::PieceCostSize>,
}

/// Load the catalog: every jewelry piece with its linked piece_costs sizes.
pub async fn load_catalog() -> Result<Vec<crate::model::CatalogPiece>, String> {
    let q = "SELECT name, kind, product_keys, thumbnail, render, sale_price, \
        (SELECT ring_size, volume_cm3, silver_g, silver_usd, gold_g, gold_usd, bronze_g, bronze_usd, wax_usd \
         FROM piece_costs WHERE design_key = $parent.id) AS sizes \
        FROM jewelry ORDER BY name";
    let mut res = DB.query(q).await.map_err(|e| e.to_string())?;
    let rows: Vec<CatalogRowDb> = res.take(0).map_err(|e| e.to_string())?;
    let thumb_path = |f: File| format!("thumb/{}", f.key().trim_start_matches('/'));
    Ok(rows
        .into_iter()
        .map(|r| crate::model::CatalogPiece {
            name: r.name,
            kind: r.kind,
            product_keys: r.product_keys,
            thumbnail: r.thumbnail.map(thumb_path),
            render: r.render.map(thumb_path),
            sale_price: r.sale_price,
            sizes: r.sizes,
        })
        .collect())
}

#[derive(SurrealValue)]
struct ListingRowDb {
    source: String,
    listing_id: String,
    title: String,
    price: f64,
    image_url: Option<String>,
}

/// Replace the cached listings for one marketplace with a fresh pull, so the
/// Listings view (and its linked rows) survive a page refresh without re-pulling.
pub async fn cache_listings(source: &str, items: &[crate::model::Listing]) -> Result<(), String> {
    DB.query("DELETE listings WHERE source = $s")
        .bind(("s", source.to_string()))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    if items.is_empty() {
        return Ok(());
    }
    let rows: Vec<ListingRowDb> = items
        .iter()
        .map(|l| ListingRowDb {
            source: l.source.clone(),
            listing_id: l.id.clone(),
            title: l.title.clone(),
            price: l.price,
            image_url: l.image_url.clone(),
        })
        .collect();
    DB.query("INSERT INTO listings $rows")
        .bind(("rows", rows))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Load every cached storefront listing.
pub async fn load_listings() -> Result<Vec<crate::model::Listing>, String> {
    let mut res = DB
        .query("SELECT source, listing_id, title, price, image_url FROM listings ORDER BY title")
        .await
        .map_err(|e| e.to_string())?;
    let rows: Vec<ListingRowDb> = res.take(0).map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| crate::model::Listing {
            source: r.source,
            id: r.listing_id,
            title: r.title,
            price: r.price,
            image_url: r.image_url,
        })
        .collect())
}
