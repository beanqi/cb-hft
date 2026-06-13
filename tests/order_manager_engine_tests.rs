use cb_hft::fix::FixEncoder;
use cb_hft::order::{
    NewOrderCommand, OrderManager, OrderStatus, OrderThreadAction, OrderThreadEngine, RiskConfig,
    RiskError, StrategyCommand, TimeInForce,
};
use cb_hft::types::{Price, ProductSpec, Qty, Side, SymbolId};

fn btc_spec() -> ProductSpec {
    ProductSpec {
        symbol_id: SymbolId(0),
        coinbase_product: "BTC-USD",
        price_scale: 100,
        qty_scale: 100_000_000,
        min_qty: Qty(10),
        min_notional: 100,
        price_tick: Price(1),
        qty_step: Qty(10),
    }
}

fn new_order(strategy_order_id: u64, price: Price, qty: Qty) -> StrategyCommand {
    StrategyCommand::NewOrder(NewOrderCommand {
        symbol_id: SymbolId(0),
        side: Side::Buy,
        price,
        qty,
        post_only: true,
        time_in_force: TimeInForce::GoodTillCancel,
        strategy_order_id,
        signal_ts_ns: 1_000,
    })
}

fn field(message: &[u8], tag: u32) -> Option<&[u8]> {
    let prefix = format!("{tag}=");
    message
        .split(|b| *b == 1)
        .find_map(|raw| raw.strip_prefix(prefix.as_bytes()))
}

#[test]
fn order_manager_accepts_new_order_and_marks_pending_new() {
    let mut manager = OrderManager::with_risk(RiskConfig::default());

    let accepted = manager
        .submit_new_order(new_order(77, Price(10_000), Qty(20)), &btc_spec())
        .unwrap();

    assert_eq!(accepted.client_order_id.as_str(), "cbhft-1");
    assert_eq!(accepted.command.strategy_order_id(), 77);
    assert_eq!(manager.status("cbhft-1"), Some(OrderStatus::PendingNew));
    assert_eq!(manager.open_order_count(SymbolId(0)), 1);
}

#[test]
fn order_manager_rejects_orders_that_violate_product_spec() {
    let mut manager = OrderManager::with_risk(RiskConfig::default());

    let result = manager.submit_new_order(new_order(1, Price(10_000), Qty(11)), &btc_spec());

    assert_eq!(result.err(), Some(RiskError::QtyNotOnStep));
    assert_eq!(manager.open_order_count(SymbolId(0)), 0);
}

#[test]
fn order_manager_enforces_max_open_orders_per_symbol() {
    let mut manager = OrderManager::with_risk(RiskConfig {
        max_open_orders_per_symbol: 1,
        ..RiskConfig::default()
    });

    manager
        .submit_new_order(new_order(1, Price(10_000), Qty(20)), &btc_spec())
        .unwrap();
    let second = manager.submit_new_order(new_order(2, Price(10_001), Qty(20)), &btc_spec());

    assert_eq!(second.err(), Some(RiskError::MaxOpenOrdersExceeded));
}

#[test]
fn order_manager_marks_pending_cancel_for_known_order() {
    let mut manager = OrderManager::with_risk(RiskConfig::default());
    let accepted = manager
        .submit_new_order(new_order(1, Price(10_000), Qty(20)), &btc_spec())
        .unwrap();

    manager
        .request_cancel(accepted.client_order_id.as_str())
        .unwrap();

    assert_eq!(
        manager.status(accepted.client_order_id.as_str()),
        Some(OrderStatus::PendingCancel)
    );
}

#[test]
fn order_thread_engine_encodes_new_order_command() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let manager = OrderManager::with_risk(RiskConfig::default());
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![btc_spec()]);

    let actions = engine.on_command(
        new_order(1, Price(6_500_012), Qty(1_000_000)),
        "20260613-12:00:00.000",
    );

    assert_eq!(actions.len(), 1);
    match &actions[0] {
        OrderThreadAction::SendFix(bytes) => {
            assert_eq!(field(bytes, 35), Some(&b"D"[..]));
            assert_eq!(field(bytes, 11), Some(&b"cbhft-1"[..]));
            assert_eq!(field(bytes, 55), Some(&b"BTC-USD"[..]));
            assert_eq!(field(bytes, 44), Some(&b"65000.12"[..]));
            assert_eq!(field(bytes, 38), Some(&b"0.01000000"[..]));
        }
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn order_thread_engine_reports_risk_reject_without_sending_fix() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let manager = OrderManager::with_risk(RiskConfig::default());
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![btc_spec()]);

    let actions = engine.on_command(
        new_order(1, Price(6_500_012), Qty(11)),
        "20260613-12:00:00.000",
    );

    assert_eq!(
        actions,
        vec![OrderThreadAction::Reject(RiskError::QtyNotOnStep)]
    );
}
