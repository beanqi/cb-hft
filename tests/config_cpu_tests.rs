use cb_hft::config::{AppConfig, ConfigError};
use cb_hft::cpu::{AffinityError, CpuAffinity};
use cb_hft::types::{Price, Qty, SymbolId};

#[test]
fn parses_minimal_app_config_from_toml() {
    let toml = r#"
[coinbase]
environment = "sandbox"
api_key_env = "COINBASE_API_KEY"
api_secret_env = "COINBASE_API_SECRET"
passphrase_env = "COINBASE_PASSPHRASE"

[threading]
order_core = 2
account_core = 3
supervisor_core = 4

[ring]
cmd_capacity = 4096
order_event_capacity = 8192
account_event_capacity = 8192

[[products]]
symbol = "BTC-USD"
market_core = 5
price_scale = 100
qty_scale = 100000000
price_tick = "0.01"
qty_step = "0.00000001"
min_qty = "0.00000001"
min_notional = 100
"#;

    let config = AppConfig::from_toml_str(toml).unwrap();

    assert_eq!(config.coinbase.environment, "sandbox");
    assert_eq!(config.threading.order_core, 2);
    assert_eq!(config.ring.cmd_capacity, 4096);
    assert_eq!(config.products.len(), 1);
    assert_eq!(config.products[0].spec.symbol_id, SymbolId(0));
    assert_eq!(config.products[0].spec.coinbase_product, "BTC-USD");
    assert_eq!(config.products[0].market_core, 5);
    assert_eq!(config.products[0].spec.price_tick, Price(1));
    assert_eq!(config.products[0].spec.qty_step, Qty(1));
}

#[test]
fn rejects_duplicate_product_symbols() {
    let toml = r#"
[coinbase]
environment = "sandbox"
api_key_env = "A"
api_secret_env = "B"
passphrase_env = "C"

[threading]
order_core = 1
account_core = 2
supervisor_core = 3

[ring]
cmd_capacity = 1
order_event_capacity = 1
account_event_capacity = 1

[[products]]
symbol = "BTC-USD"
market_core = 4
price_scale = 100
qty_scale = 100
price_tick = "0.01"
qty_step = "0.01"
min_qty = "0.01"
min_notional = 1

[[products]]
symbol = "BTC-USD"
market_core = 5
price_scale = 100
qty_scale = 100
price_tick = "0.01"
qty_step = "0.01"
min_qty = "0.01"
min_notional = 1
"#;

    assert!(matches!(
        AppConfig::from_toml_str(toml),
        Err(ConfigError::DuplicateProductSymbol(_))
    ));
}

#[test]
fn cpu_affinity_noop_rejects_obviously_invalid_core_ids() {
    assert_eq!(
        CpuAffinity::pin_current_thread(usize::MAX),
        Err(AffinityError::InvalidCoreId)
    );
}
