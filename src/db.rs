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
            Ok(())
        })
        .await
        .map(|_| ())
}

#[derive(SurrealValue)]
struct OrderCacheRow {
    source: String,
    order_number: String,
    payload: String,
}

/// JSON payloads of orders cached within the last day (empty = stale/miss).
pub async fn load_cached_orders() -> Result<Vec<String>, String> {
    let mut res = DB
        .query("SELECT VALUE payload FROM orders WHERE fetched_at > time::now() - 1d")
        .await
        .map_err(|e| e.to_string())?;
    res.take(0).map_err(|e| e.to_string())
}

/// Replace the order cache with the given set (fetched_at set to now).
pub async fn save_orders(orders: &[crate::model::Order]) -> Result<(), String> {
    let rows: Vec<OrderCacheRow> = orders
        .iter()
        .map(|o| OrderCacheRow {
            source: match o.source {
                crate::model::OrderSource::Shopify => "shopify".to_string(),
                crate::model::OrderSource::Etsy => "etsy".to_string(),
            },
            order_number: o.order_number.clone(),
            payload: serde_json::to_string(o).unwrap_or_default(),
        })
        .collect();
    DB.query("DELETE orders; INSERT INTO orders $rows")
        .bind(("rows", rows))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
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

/// Keys are our own `<hash>.<ext>` slugs; reject anything else (the key is
/// interpolated into the file literal).
fn safe_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')
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

/// Load the catalog: every jewelry piece with its linked piece_costs sizes.
pub async fn load_catalog() -> Result<Vec<crate::model::CatalogPiece>, String> {
    let q = "SELECT name, kind, product_keys, \
        (SELECT ring_size, volume_cm3, silver_g, silver_usd, gold_g, gold_usd, bronze_g, bronze_usd, wax_usd \
         FROM piece_costs WHERE design_key = $parent.id) AS sizes \
        FROM jewelry ORDER BY name";
    let mut res = DB.query(q).await.map_err(|e| e.to_string())?;
    let pieces: Vec<crate::model::CatalogPiece> = res.take(0).map_err(|e| e.to_string())?;
    Ok(pieces)
}
