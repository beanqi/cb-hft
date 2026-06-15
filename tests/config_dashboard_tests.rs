use cb_hft::config::{AppConfig, DashboardSecrets, persist_dashboard_update};
use cb_hft::maker::TrendConfig;
use cb_hft::types::{Qty, SymbolId};

const CFG: &str = r#"
[coinbase]
environment = "sandbox"
api_key_env = "COINBASE_API_KEY"
api_secret_env = "COINBASE_API_SECRET"
passphrase_env = "COINBASE_PASSPHRASE"
api_key = "file-key"
api_secret = "file-secret"
passphrase = "file-pass"

[threading]
order_core = 2
account_core = 3
supervisor_core = 4

[ring]
cmd_capacity = 4096
order_event_capacity = 8192
account_event_capacity = 8192

[strategy]
trend_window_ms = 1500
min_window_notional = 6000000
strong_score_x100 = 175
trade_weight_x100 = 110
count_weight_x100 = 60
obi_weight_x100 = 90
micro_weight_x100 = 30
quote_tick_offset_ticks = 1
requote_ticks = 2

[[products]]
symbol = "BTC-USD"
market_core = 5
price_scale = 100
qty_scale = 100000000
price_tick = "0.01"
qty_step = "0.00000001"
min_qty = "0.00000001"
min_notional = 100
quote_qty = "0.02"
"#;

#[test]
fn config_parses_file_credentials_and_strategy_params() {
    let cfg = AppConfig::from_toml_str(CFG).unwrap();

    assert_eq!(cfg.coinbase.api_key.as_deref(), Some("file-key"));
    assert_eq!(cfg.coinbase.api_secret.as_deref(), Some("file-secret"));
    assert_eq!(cfg.coinbase.passphrase.as_deref(), Some("file-pass"));
    assert_eq!(cfg.strategy.trend.window_ns, 1_500_000_000);
    assert_eq!(cfg.strategy.trend.strong_score_x100, 175);
    assert_eq!(cfg.strategy.quote_tick_offset_ticks, 1);
    assert_eq!(cfg.strategy.requote_ticks, 2);
    assert_eq!(cfg.products[0].maker_quote_qty, Qty(2_000_000));
    assert_eq!(cfg.products[0].spec.symbol_id, SymbolId(0));
}

#[test]
fn config_defaults_strategy_when_missing() {
    let mut cfg_text = CFG.to_string();
    cfg_text = cfg_text.replace(
        "api_key = \"file-key\"\napi_secret = \"file-secret\"\npassphrase = \"file-pass\"\n",
        "",
    );
    cfg_text = cfg_text.replace("\n[strategy]\ntrend_window_ms = 1500\nmin_window_notional = 6000000\nstrong_score_x100 = 175\ntrade_weight_x100 = 110\ncount_weight_x100 = 60\nobi_weight_x100 = 90\nmicro_weight_x100 = 30\nquote_tick_offset_ticks = 1\nrequote_ticks = 2\n", "");
    cfg_text = cfg_text.replace("quote_qty = \"0.02\"\n", "");
    let cfg = AppConfig::from_toml_str(&cfg_text).unwrap();

    assert_eq!(cfg.strategy.trend, TrendConfig::default());
    assert_eq!(cfg.strategy.quote_tick_offset_ticks, 1);
    assert_eq!(cfg.strategy.requote_ticks, 1);
    assert_eq!(cfg.products[0].maker_quote_qty, Qty(10_000_000));
}

#[test]
fn dashboard_update_persists_secrets_and_strategy_to_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, CFG).unwrap();

    persist_dashboard_update(
        &path,
        DashboardSecrets {
            api_key: Some("new-key".to_string()),
            api_secret: Some("new-secret".to_string()),
            passphrase: Some("new-pass".to_string()),
            trend_window_ms: Some(2500),
            min_window_notional: Some(7000000),
            strong_score_x100: Some(190),
            quote_qty_by_symbol: vec![("BTC-USD".to_string(), "0.03".to_string())],
        },
    )
    .unwrap();

    let written = std::fs::read_to_string(&path).unwrap();
    let cfg = AppConfig::from_toml_str(&written).unwrap();
    assert_eq!(cfg.coinbase.api_key.as_deref(), Some("new-key"));
    assert_eq!(cfg.coinbase.api_secret.as_deref(), Some("new-secret"));
    assert_eq!(cfg.coinbase.passphrase.as_deref(), Some("new-pass"));
    assert_eq!(cfg.strategy.trend.window_ns, 2_500_000_000);
    assert_eq!(cfg.strategy.trend.min_window_notional, 7_000_000);
    assert_eq!(cfg.strategy.trend.strong_score_x100, 190);
    assert_eq!(cfg.products[0].maker_quote_qty, Qty(3_000_000));
}
