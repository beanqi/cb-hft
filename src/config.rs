use std::collections::HashSet;

use serde::Deserialize;

use crate::types::{Price, ProductSpec, Qty, SymbolId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub coinbase: CoinbaseConfig,
    pub threading: ThreadingConfig,
    pub ring: RingConfig,
    pub products: Vec<ProductConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CoinbaseConfig {
    pub environment: String,
    pub api_key_env: String,
    pub api_secret_env: String,
    pub passphrase_env: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct ThreadingConfig {
    pub order_core: usize,
    pub account_core: usize,
    pub supervisor_core: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct RingConfig {
    pub cmd_capacity: usize,
    pub order_event_capacity: usize,
    pub account_event_capacity: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductConfig {
    pub spec: ProductSpec,
    pub market_core: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Toml(String),
    DuplicateProductSymbol(String),
    InvalidDecimal(String),
}

#[derive(Debug, Deserialize)]
struct RawAppConfig {
    coinbase: CoinbaseConfig,
    threading: ThreadingConfig,
    ring: RingConfig,
    products: Vec<RawProductConfig>,
}

#[derive(Debug, Deserialize)]
struct RawProductConfig {
    symbol: String,
    market_core: usize,
    price_scale: i64,
    qty_scale: i64,
    price_tick: String,
    qty_step: String,
    min_qty: String,
    min_notional: i64,
}

impl AppConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let raw: RawAppConfig =
            toml::from_str(input).map_err(|err| ConfigError::Toml(err.to_string()))?;
        let mut seen = HashSet::new();
        let mut products = Vec::with_capacity(raw.products.len());

        for (idx, raw_product) in raw.products.into_iter().enumerate() {
            if !seen.insert(raw_product.symbol.clone()) {
                return Err(ConfigError::DuplicateProductSymbol(raw_product.symbol));
            }
            let symbol_static: &'static str = Box::leak(raw_product.symbol.into_boxed_str());
            let price_tick =
                Price::parse_scaled(raw_product.price_tick.as_bytes(), raw_product.price_scale)
                    .map_err(|_| ConfigError::InvalidDecimal("price_tick".to_string()))?;
            let qty_step =
                Qty::parse_scaled(raw_product.qty_step.as_bytes(), raw_product.qty_scale)
                    .map_err(|_| ConfigError::InvalidDecimal("qty_step".to_string()))?;
            let min_qty = Qty::parse_scaled(raw_product.min_qty.as_bytes(), raw_product.qty_scale)
                .map_err(|_| ConfigError::InvalidDecimal("min_qty".to_string()))?;

            products.push(ProductConfig {
                spec: ProductSpec {
                    symbol_id: SymbolId(idx as u16),
                    coinbase_product: symbol_static,
                    price_scale: raw_product.price_scale,
                    qty_scale: raw_product.qty_scale,
                    min_qty,
                    min_notional: raw_product.min_notional,
                    price_tick,
                    qty_step,
                },
                market_core: raw_product.market_core,
            });
        }

        Ok(Self {
            coinbase: raw.coinbase,
            threading: raw.threading,
            ring: raw.ring,
            products,
        })
    }
}
