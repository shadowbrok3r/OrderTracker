#!/usr/bin/with-contenv bashio

# Read add-on options and export as environment variables
CONFIG_PATH=/data/options.json

if [ -f "$CONFIG_PATH" ]; then
    for key in SURREAL_URL SHOPIFY_URL SHOPIFY_ACCESS_TOKEN ETSY_KEYSTRING ETSY_SECRET ETSY_SHOP_ID; do
        val=$(bashio::jq "$CONFIG_PATH" ".$key // empty")
        if [ -n "$val" ]; then
            export "$key=$val"
            bashio::log.info "Set $key"
        fi
    done
else
    bashio::log.warning "No options.json found at $CONFIG_PATH"
fi

bashio::log.info "Starting Order Tracker on port 8099..."
exec /app/order_tracker --addr 0.0.0.0 --port 8099
