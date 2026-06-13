use cb_hft::fix::FixEncoder;
use cb_hft::order::{
    CancelOrderCommand, NewOrderCommand, OrderManager, OrderStatus, OrderThreadAction,
    OrderThreadEngine, RiskConfig, RiskError, StrategyCommand, TimeInForce,
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

fn eth_spec() -> ProductSpec {
    ProductSpec {
        symbol_id: SymbolId(1),
        coinbase_product: "ETH-USD",
        price_scale: 100,
        qty_scale: 100_000_000,
        min_qty: Qty(10),
        min_notional: 100,
        price_tick: Price(1),
        qty_step: Qty(10),
    }
}

fn new_order(symbol_id: SymbolId, strategy_order_id: u64, price: Price) -> StrategyCommand {
    StrategyCommand::NewOrder(NewOrderCommand {
        symbol_id,
        side: Side::Buy,
        price,
        qty: Qty(1_000_000),
        post_only: true,
        time_in_force: TimeInForce::GoodTillCancel,
        strategy_order_id,
        signal_ts_ns: 1_000,
    })
}

fn cancel_order(symbol_id: SymbolId, client_order_id: u64) -> StrategyCommand {
    StrategyCommand::CancelOrder(CancelOrderCommand {
        symbol_id,
        client_order_id,
        strategy_order_id: 99,
        signal_ts_ns: 2_000,
    })
}

fn field(message: &[u8], tag: u32) -> Option<&[u8]> {
    let prefix = format!("{tag}=");
    message
        .split(|b| *b == 1)
        .find_map(|raw| raw.strip_prefix(prefix.as_bytes()))
}

#[test]
fn order_thread_engine_encodes_cancel_for_known_client_order() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let manager = OrderManager::with_risk(RiskConfig::default());
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![btc_spec()]);

    engine.on_command(
        new_order(SymbolId(0), 1, Price(6_500_012)),
        "20260613-12:00:00.000",
    );
    let actions = engine.on_command(cancel_order(SymbolId(0), 1), "20260613-12:00:01.000");

    assert_eq!(actions.len(), 1);
    match &actions[0] {
        OrderThreadAction::SendFix(bytes) => {
            assert_eq!(field(bytes, 35), Some(&b"F"[..]));
            assert_eq!(field(bytes, 11), Some(&b"cbhft-2"[..]));
            assert_eq!(field(bytes, 41), Some(&b"cbhft-1"[..]));
            assert_eq!(field(bytes, 55), Some(&b"BTC-USD"[..]));
            assert_eq!(field(bytes, 54), Some(&b"1"[..]));
        }
        other => panic!("unexpected action: {other:?}"),
    }
    assert_eq!(
        engine.manager().status("cbhft-1"),
        Some(OrderStatus::PendingCancel)
    );
}

#[test]
fn order_thread_engine_rejects_cancel_for_unknown_order() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let manager = OrderManager::with_risk(RiskConfig::default());
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![btc_spec()]);

    let actions = engine.on_command(cancel_order(SymbolId(0), 404), "20260613-12:00:01.000");

    assert_eq!(
        actions,
        vec![OrderThreadAction::Reject(RiskError::UnknownClientOrderId)]
    );
}

#[test]
fn order_thread_engine_cancel_all_generates_cancels_for_symbol_only() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let manager = OrderManager::with_risk(RiskConfig::default());
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![btc_spec(), eth_spec()]);

    engine.on_command(
        new_order(SymbolId(0), 1, Price(6_500_012)),
        "20260613-12:00:00.000",
    );
    engine.on_command(
        new_order(SymbolId(0), 2, Price(6_500_013)),
        "20260613-12:00:00.001",
    );
    engine.on_command(
        new_order(SymbolId(1), 3, Price(3_000_000)),
        "20260613-12:00:00.002",
    );

    let actions = engine.on_command(
        StrategyCommand::CancelAll {
            symbol_id: SymbolId(0),
            signal_ts_ns: 3_000,
        },
        "20260613-12:00:02.000",
    );

    assert_eq!(actions.len(), 2);
    let orig_ids: Vec<_> = actions
        .iter()
        .map(|action| match action {
            OrderThreadAction::SendFix(bytes) => field(bytes, 41).unwrap().to_vec(),
            other => panic!("unexpected action: {other:?}"),
        })
        .collect();
    assert_eq!(orig_ids, vec![b"cbhft-1".to_vec(), b"cbhft-2".to_vec()]);
    assert_eq!(
        engine.manager().status("cbhft-1"),
        Some(OrderStatus::PendingCancel)
    );
    assert_eq!(
        engine.manager().status("cbhft-2"),
        Some(OrderStatus::PendingCancel)
    );
    assert_eq!(
        engine.manager().status("cbhft-3"),
        Some(OrderStatus::PendingNew)
    );
}
