use cb_hft::config::AppConfig;
use cb_hft::runtime::{RuntimeOptions, run};

const CFG_WITHOUT_KEYS: &str = r#"
[coinbase]
environment = "sandbox"
api_key_env = "CB_HFT_TEST_MISSING_API_KEY"
api_secret_env = "CB_HFT_TEST_MISSING_API_SECRET"
passphrase_env = "CB_HFT_TEST_MISSING_PASSPHRASE"

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

#[test]
fn config_reports_missing_and_present_file_credentials() {
    let cfg = AppConfig::from_toml_str(CFG_WITHOUT_KEYS).unwrap();
    assert_eq!(
        cfg.missing_credential_names(),
        vec![
            "CB_HFT_TEST_MISSING_API_KEY".to_string(),
            "CB_HFT_TEST_MISSING_API_SECRET".to_string(),
            "CB_HFT_TEST_MISSING_PASSPHRASE".to_string(),
        ]
    );

    let with_keys = CFG_WITHOUT_KEYS.replace(
        "passphrase_env = \"CB_HFT_TEST_MISSING_PASSPHRASE\"",
        "passphrase_env = \"CB_HFT_TEST_MISSING_PASSPHRASE\"\napi_key = \"k\"\napi_secret = \"s\"\npassphrase = \"p\"",
    );
    let cfg = AppConfig::from_toml_str(&with_keys).unwrap();
    assert!(cfg.missing_credential_names().is_empty());
}

#[test]
fn live_runtime_without_credentials_returns_ok_without_opening_fix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, CFG_WITHOUT_KEYS).unwrap();

    let result = run(RuntimeOptions {
        config_path: path.to_string_lossy().to_string(),
        dry_run: false,
        market_data: true,
        order_entry: true,
        account: false,
        once: true,
        dashboard: false,
    });

    assert!(result.is_ok());
}
