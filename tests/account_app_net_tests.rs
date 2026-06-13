use cb_hft::account::{AccountSnapshot, BalanceBook, UserFeedEvent};
use cb_hft::app::AppTopology;
use cb_hft::config::AppConfig;
use cb_hft::event::{BalanceEvent, ExchangeEvent};
use cb_hft::net::{SocketTuning, TcpNoDelay};
use cb_hft::supervisor::{ShutdownReason, ShutdownSignal};
use cb_hft::telemetry::LatencySample;
use cb_hft::types::{AssetId, Qty};

fn config() -> AppConfig {
    AppConfig::from_toml_str(
        r#"
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
cmd_capacity = 16
order_event_capacity = 32
account_event_capacity = 32

[[products]]
symbol = "BTC-USD"
market_core = 5
price_scale = 100
qty_scale = 100000000
price_tick = "0.01"
qty_step = "0.00000001"
min_qty = "0.00000001"
min_notional = 100

[[products]]
symbol = "ETH-USD"
market_core = 6
price_scale = 100
qty_scale = 100000000
price_tick = "0.01"
qty_step = "0.00000001"
min_qty = "0.00000001"
min_notional = 100
"#,
    )
    .unwrap()
}

#[test]
fn balance_book_applies_snapshot_and_user_feed_updates() {
    let mut book = BalanceBook::default();
    let usd = AssetId::from_static("USD");
    let btc = AssetId::from_static("BTC");

    book.apply_snapshot(AccountSnapshot::new(vec![
        BalanceEvent {
            asset_id: usd.clone(),
            total: Qty(1_000),
            available: Qty(900),
            hold: Qty(100),
            update_ts_ns: 10,
            recv_ts_ns: 11,
        },
        BalanceEvent {
            asset_id: btc.clone(),
            total: Qty(2),
            available: Qty(1),
            hold: Qty(1),
            update_ts_ns: 10,
            recv_ts_ns: 11,
        },
    ]));

    book.apply_user_feed_event(UserFeedEvent::Balance(BalanceEvent {
        asset_id: usd.clone(),
        total: Qty(1_100),
        available: Qty(1_000),
        hold: Qty(100),
        update_ts_ns: 20,
        recv_ts_ns: 21,
    }));

    assert_eq!(book.balance(&usd).unwrap().total, Qty(1_100));
    assert_eq!(book.balance(&btc).unwrap().total, Qty(2));
}

#[test]
fn user_feed_balance_event_converts_to_exchange_event() {
    let event = UserFeedEvent::Balance(BalanceEvent {
        asset_id: AssetId::from_static("USD"),
        total: Qty(10),
        available: Qty(9),
        hold: Qty(1),
        update_ts_ns: 1,
        recv_ts_ns: 2,
    });

    assert!(matches!(
        event.into_exchange_event(),
        ExchangeEvent::Balance(_)
    ));
}

#[test]
fn app_topology_creates_per_symbol_ring_counts_from_config() {
    let topology = AppTopology::from_config(&config()).unwrap();

    assert_eq!(topology.symbol_count(), 2);
    assert_eq!(topology.command_rings().len(), 2);
    assert_eq!(topology.order_event_rings().len(), 2);
    assert_eq!(topology.account_event_rings().len(), 2);
}

#[test]
fn shutdown_signal_latches_first_reason() {
    let signal = ShutdownSignal::default();

    assert!(!signal.is_shutdown());
    signal.request(ShutdownReason::UserRequested);
    signal.request(ShutdownReason::FatalError);

    assert!(signal.is_shutdown());
    assert_eq!(signal.reason(), Some(ShutdownReason::UserRequested));
}

#[test]
fn socket_tuning_builder_records_requested_options() {
    let tuning = SocketTuning::default().with_tcp_no_delay(TcpNoDelay::Enabled);

    assert_eq!(tuning.tcp_no_delay, TcpNoDelay::Enabled);
}

#[test]
fn latency_sample_computes_elapsed_nanos() {
    let sample = LatencySample::new("parse", 100, 175);

    assert_eq!(sample.name(), "parse");
    assert_eq!(sample.elapsed_ns(), 75);
}
