use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use crate::maker::TrendConfig;
use crate::types::{Price, ProductSpec, Qty, SymbolId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub coinbase: CoinbaseConfig,
    pub threading: ThreadingConfig,
    pub ring: RingConfig,
    pub strategy: StrategyConfig,
    pub products: Vec<ProductConfig>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CoinbaseConfig {
    pub environment: String,
    pub api_key_env: String,
    pub api_secret_env: String,
    pub passphrase_env: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_secret: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrategyConfig {
    pub trend: TrendConfig,
    pub quote_tick_offset_ticks: i64,
    pub requote_ticks: i64,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            trend: TrendConfig::default(),
            quote_tick_offset_ticks: 1,
            requote_ticks: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductConfig {
    pub spec: ProductSpec,
    pub market_core: usize,
    pub maker_quote_qty: Qty,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Toml(String),
    DuplicateProductSymbol(String),
    InvalidDecimal(String),
    Io(String),
}

#[derive(Debug, Deserialize)]
struct RawAppConfig {
    coinbase: CoinbaseConfig,
    threading: ThreadingConfig,
    ring: RingConfig,
    #[serde(default)]
    strategy: RawStrategyConfig,
    products: Vec<RawProductConfig>,
}

#[derive(Debug, Deserialize)]
struct RawStrategyConfig {
    #[serde(default = "default_window_ms")]
    trend_window_ms: u64,
    #[serde(default = "default_min_window_notional")]
    min_window_notional: i64,
    #[serde(default = "default_strong_score")]
    strong_score_x100: i64,
    #[serde(default = "default_trade_weight")]
    trade_weight_x100: i64,
    #[serde(default = "default_count_weight")]
    count_weight_x100: i64,
    #[serde(default = "default_obi_weight")]
    obi_weight_x100: i64,
    #[serde(default = "default_micro_weight")]
    micro_weight_x100: i64,
    #[serde(default = "default_tick_count")]
    quote_tick_offset_ticks: i64,
    #[serde(default = "default_tick_count")]
    requote_ticks: i64,
}

impl Default for RawStrategyConfig {
    fn default() -> Self {
        Self {
            trend_window_ms: default_window_ms(),
            min_window_notional: default_min_window_notional(),
            strong_score_x100: default_strong_score(),
            trade_weight_x100: default_trade_weight(),
            count_weight_x100: default_count_weight(),
            obi_weight_x100: default_obi_weight(),
            micro_weight_x100: default_micro_weight(),
            quote_tick_offset_ticks: default_tick_count(),
            requote_ticks: default_tick_count(),
        }
    }
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
    #[serde(default)]
    quote_qty: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DashboardSecrets {
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub passphrase: Option<String>,
    pub trend_window_ms: Option<u64>,
    pub min_window_notional: Option<i64>,
    pub strong_score_x100: Option<i64>,
    pub quote_qty_by_symbol: Vec<(String, String)>,
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
            let maker_quote_qty = raw_product
                .quote_qty
                .as_deref()
                .map(|value| Qty::parse_scaled(value.as_bytes(), raw_product.qty_scale))
                .transpose()
                .map_err(|_| ConfigError::InvalidDecimal("quote_qty".to_string()))?
                .unwrap_or(Qty(10_000_000));

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
                maker_quote_qty,
            });
        }

        Ok(Self {
            coinbase: raw.coinbase,
            threading: raw.threading,
            ring: raw.ring,
            strategy: StrategyConfig {
                trend: TrendConfig {
                    window_ns: raw.strategy.trend_window_ms.saturating_mul(1_000_000),
                    min_window_notional: raw.strategy.min_window_notional as i128,
                    strong_score_x100: raw.strategy.strong_score_x100,
                    trade_weight_x100: raw.strategy.trade_weight_x100,
                    count_weight_x100: raw.strategy.count_weight_x100,
                    obi_weight_x100: raw.strategy.obi_weight_x100,
                    micro_weight_x100: raw.strategy.micro_weight_x100,
                },
                quote_tick_offset_ticks: raw.strategy.quote_tick_offset_ticks,
                requote_ticks: raw.strategy.requote_ticks,
            },
            products,
        })
    }

    pub fn missing_credential_names(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if std::env::var(&self.coinbase.api_key_env).is_err() && self.coinbase.api_key.is_none() {
            missing.push(self.coinbase.api_key_env.clone());
        }
        if std::env::var(&self.coinbase.api_secret_env).is_err()
            && self.coinbase.api_secret.is_none()
        {
            missing.push(self.coinbase.api_secret_env.clone());
        }
        if std::env::var(&self.coinbase.passphrase_env).is_err()
            && self.coinbase.passphrase.is_none()
        {
            missing.push(self.coinbase.passphrase_env.clone());
        }
        missing
    }
}

pub fn persist_dashboard_update(
    path: impl AsRef<Path>,
    update: DashboardSecrets,
) -> Result<(), ConfigError> {
    let path = path.as_ref();
    let mut text = std::fs::read_to_string(path).map_err(|err| ConfigError::Io(err.to_string()))?;
    if let Some(api_key) = update.api_key {
        text = set_key_in_section(&text, "coinbase", "api_key", &quote(&api_key));
    }
    if let Some(api_secret) = update.api_secret {
        text = set_key_in_section(&text, "coinbase", "api_secret", &quote(&api_secret));
    }
    if let Some(passphrase) = update.passphrase {
        text = set_key_in_section(&text, "coinbase", "passphrase", &quote(&passphrase));
    }
    if let Some(v) = update.trend_window_ms {
        text = set_key_in_section(&text, "strategy", "trend_window_ms", &v.to_string());
    }
    if let Some(v) = update.min_window_notional {
        text = set_key_in_section(&text, "strategy", "min_window_notional", &v.to_string());
    }
    if let Some(v) = update.strong_score_x100 {
        text = set_key_in_section(&text, "strategy", "strong_score_x100", &v.to_string());
    }
    for (symbol, qty) in update.quote_qty_by_symbol {
        text = set_product_key(&text, &symbol, "quote_qty", &quote(&qty));
    }
    std::fs::write(path, text).map_err(|err| ConfigError::Io(err.to_string()))
}

fn set_key_in_section(text: &str, section: &str, key: &str, value: &str) -> String {
    let header = format!("[{section}]");
    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
    let Some(start) = lines.iter().position(|line| line.trim() == header) else {
        lines.push(String::new());
        lines.push(header);
        lines.push(format!("{key} = {value}"));
        return lines.join("\n") + "\n";
    };
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .map(|pos| start + 1 + pos)
        .unwrap_or(lines.len());
    if let Some(idx) =
        (start + 1..end).find(|&idx| lines[idx].trim_start().starts_with(&format!("{key} =")))
    {
        lines[idx] = format!("{key} = {value}");
    } else {
        lines.insert(end, format!("{key} = {value}"));
    }
    lines.join("\n") + "\n"
}

fn set_product_key(text: &str, symbol: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
    let mut idx = 0;
    while idx < lines.len() {
        if lines[idx].trim() == "[[products]]" {
            let end = lines[idx + 1..]
                .iter()
                .position(|line| line.trim_start().starts_with('['))
                .map(|pos| idx + 1 + pos)
                .unwrap_or(lines.len());
            let is_symbol = (idx + 1..end)
                .any(|line_idx| lines[line_idx].trim() == format!("symbol = {}", quote(symbol)));
            if is_symbol {
                if let Some(key_idx) = (idx + 1..end).find(|&line_idx| {
                    lines[line_idx]
                        .trim_start()
                        .starts_with(&format!("{key} ="))
                }) {
                    lines[key_idx] = format!("{key} = {value}");
                } else {
                    lines.insert(end, format!("{key} = {value}"));
                }
                return lines.join("\n") + "\n";
            }
            idx = end;
        } else {
            idx += 1;
        }
    }
    lines.join("\n") + "\n"
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn default_window_ms() -> u64 {
    2000
}
fn default_min_window_notional() -> i64 {
    5_000_000
}
fn default_strong_score() -> i64 {
    150
}
fn default_trade_weight() -> i64 {
    100
}
fn default_count_weight() -> i64 {
    50
}
fn default_obi_weight() -> i64 {
    80
}
fn default_micro_weight() -> i64 {
    40
}
fn default_tick_count() -> i64 {
    1
}
