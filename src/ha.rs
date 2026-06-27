//! Push OrderTracker metrics to Home Assistant via the Supervisor proxy
//! (`http://supervisor/core/api`, authed with SUPERVISOR_TOKEN — granted by
//! `homeassistant_api: true` in the add-on config). Server-only.

use crate::model::{MetalType, Order, OrderSource};

fn supervisor() -> Option<(String, reqwest::Client)> {
    let token = std::env::var("SUPERVISOR_TOKEN").ok().filter(|t| !t.is_empty())?;
    Some((token, reqwest::Client::new()))
}

fn src_str(s: &OrderSource) -> &'static str {
    match s {
        OrderSource::Shopify => "shopify",
        OrderSource::Etsy => "etsy",
        OrderSource::Custom => "custom",
    }
}

async fn push_state(
    client: &reqwest::Client,
    token: &str,
    entity: &str,
    state: String,
    attributes: serde_json::Value,
) {
    let url = format!("http://supervisor/core/api/states/{entity}");
    let body = serde_json::json!({ "state": state, "attributes": attributes });
    if let Err(e) = client.post(&url).bearer_auth(token).json(&body).send().await {
        crate::log::app_log("INFO", format!("HA push {entity} failed: {e}"));
    }
}

async fn fire_event(client: &reqwest::Client, token: &str, event: &str, data: serde_json::Value) {
    let url = format!("http://supervisor/core/api/events/{event}");
    let _ = client.post(&url).bearer_auth(token).json(&data).send().await;
}

/// Read a Home Assistant entity's state string via the Supervisor proxy.
async fn read_state(client: &reqwest::Client, token: &str, entity: &str) -> Option<String> {
    let url = format!("http://supervisor/core/api/states/{entity}");
    let resp = client.get(&url).bearer_auth(token).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    v.get("state").and_then(|s| s.as_str()).map(|s| s.to_string())
}

/// Poll the kiln entity and auto-advance burnout stages on firing transitions.
/// On kiln-on: orders in "Invested" -> "Burnout". On kiln-off: "Burnout" ->
/// "Finishing". `prev_firing` carries the last observed firing state across
/// calls; the first observation only records state (no advance). No-op without
/// the Supervisor or when KILN_ENTITY is blank.
pub async fn kiln_poll(prev_firing: &mut Option<bool>) {
    let Some((token, client)) = supervisor() else {
        return;
    };
    let entity = std::env::var("KILN_ENTITY").unwrap_or_else(|_| "sensor.kiln_mode".to_string());
    if entity.trim().is_empty() {
        return;
    }
    let firing_states = std::env::var("KILN_FIRING_STATE").unwrap_or_else(|_| "firing".to_string());
    let active: Vec<String> = firing_states
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let Some(state) = read_state(&client, &token, &entity).await else {
        return;
    };
    let sl = state.to_lowercase();
    let firing = active.iter().any(|a| sl.contains(a));

    match (*prev_firing, firing) {
        (Some(false), true) => {
            if let Ok(n) = crate::db::advance_stage("Invested", "Burnout").await {
                if n > 0 {
                    crate::log::app_log("INFO", format!("Kiln firing: {n} order(s) Invested -> Burnout"));
                }
            }
        }
        (Some(true), false) => {
            if let Ok(n) = crate::db::advance_stage("Burnout", "Finishing").await {
                if n > 0 {
                    crate::log::app_log("INFO", format!("Kiln off: {n} order(s) Burnout -> Finishing"));
                }
            }
        }
        _ => {}
    }
    *prev_firing = Some(firing);
}

/// Push order sensors to HA and fire `ordertracker_new_order` for orders not yet
/// announced. No-op when not running under the Supervisor.
pub async fn push_orders(orders: &[Order]) {
    let Some((token, client)) = supervisor() else {
        return;
    };

    let open: Vec<&Order> = orders.iter().filter(|o| !o.archived && !o.completed).collect();
    let urgent = open
        .iter()
        .filter(|o| (0..=3).contains(&o.days_until_due()))
        .count();
    let overdue = open.iter().filter(|o| o.days_until_due() < 0).count();
    let revenue: f64 = open.iter().map(|o| o.total_price).sum();

    let silver: f64 = match crate::db::load_catalog().await {
        Ok(cat) => open
            .iter()
            .flat_map(|o| o.items.iter())
            .filter(|i| i.metal_type != MetalType::Bronze)
            .filter_map(|i| crate::model::lookup_piece_cost(i, &cat).map(|cw| cw.weight_g * i.quantity as f64))
            .sum(),
        Err(_) => 0.0,
    };

    let list: Vec<serde_json::Value> = open
        .iter()
        .map(|o| {
            serde_json::json!({
                "order": o.order_number,
                "customer": o.customer_name,
                "source": src_str(&o.source),
                "due": o.due_date.format("%Y-%m-%d").to_string(),
                "days_left": o.days_until_due(),
                "total": o.total_price,
                "stage": o.stage.clone().unwrap_or_default(),
                "notes": o.notes.clone().unwrap_or_default(),
                "items": o.items.iter().map(|i| i.name.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();

    push_state(&client, &token, "sensor.ordertracker_open_orders", open.len().to_string(),
        serde_json::json!({ "friendly_name": "Open orders", "icon": "mdi:package-variant-closed", "orders": list })).await;
    push_state(&client, &token, "sensor.ordertracker_urgent", urgent.to_string(),
        serde_json::json!({ "friendly_name": "Urgent orders", "icon": "mdi:alert" })).await;
    push_state(&client, &token, "sensor.ordertracker_overdue", overdue.to_string(),
        serde_json::json!({ "friendly_name": "Overdue orders", "icon": "mdi:alert-octagon" })).await;
    push_state(&client, &token, "sensor.ordertracker_silver_to_buy", format!("{silver:.1}"),
        serde_json::json!({ "friendly_name": "Silver to buy", "unit_of_measurement": "g", "device_class": "weight", "state_class": "measurement", "icon": "mdi:scale" })).await;
    push_state(&client, &token, "sensor.ordertracker_open_revenue", format!("{revenue:.2}"),
        serde_json::json!({ "friendly_name": "Open order revenue", "unit_of_measurement": "USD", "state_class": "measurement", "icon": "mdi:cash" })).await;

    if let Ok(seen) = crate::db::load_seen_keys().await {
        // First run adopts all current orders silently (no event flood); the
        // __baseline__ sentinel records initialization even with zero orders.
        let initialized = seen.contains("__baseline__");
        let mut newly = Vec::new();
        for o in &open {
            let key = o.state_key();
            if seen.contains(&key) {
                continue;
            }
            if initialized {
                fire_event(&client, &token, "ordertracker_new_order", serde_json::json!({
                    "order": o.order_number,
                    "customer": o.customer_name,
                    "source": src_str(&o.source),
                    "due": o.due_date.format("%Y-%m-%d").to_string(),
                    "total": o.total_price,
                    "items": o.items.iter().map(|i| i.name.clone()).collect::<Vec<_>>(),
                })).await;
            }
            newly.push(key);
        }
        if !initialized {
            newly.push("__baseline__".to_string());
        }
        if !newly.is_empty() {
            let _ = crate::db::mark_seen(&newly).await;
        }
    }
}
