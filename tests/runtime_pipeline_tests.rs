use cb_hft::event::{ExchangeEvent, FillEvent};
use cb_hft::fix::FixEncoder;
use cb_hft::market::MarketEvent;
use cb_hft::order::{
    NewOrderCommand, OrderEvent, OrderEventSource, OrderManager, OrderStatus, OrderThreadAction,
    OrderThreadEngine, RiskConfig, StrategyCommand, TimeInForce,
};
use cb_hft::runtime::{RuntimePipeline, RuntimePipelineConfig, RuntimePipelineStep};
use cb_hft::strategy::QuoteOnFirstL1Strategy;
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

fn l1_event() -> MarketEvent {
    MarketEvent::L1 {
        symbol_id: SymbolId(0),
        recv_ts_ns: 1_000,
        bid_px: Price(6_500_000),
        bid_qty: Qty(2_000_000),
        ask_px: Price(6_500_012),
        ask_qty: Qty(1_000_000),
        sequence: 7,
    }
}

fn quoted_order() -> StrategyCommand {
    StrategyCommand::NewOrder(NewOrderCommand {
        symbol_id: SymbolId(0),
        side: Side::Buy,
        price: Price(6_500_012),
        qty: Qty(1_000_000),
        post_only: true,
        time_in_force: TimeInForce::GoodTillCancel,
        strategy_order_id: 99,
        signal_ts_ns: 1_000,
    })
}

fn order_event(status: OrderStatus) -> OrderEvent {
    OrderEvent {
        symbol_id: SymbolId(0),
        client_order_id: "cbhft-1".to_string(),
        exchange_order_id: "ex-1".to_string(),
        exec_id: format!("exec-{status:?}"),
        status,
        side: Side::Buy,
        price: Price(6_500_012),
        original_qty: Qty(1_000_000),
        remaining_qty: if matches!(status, OrderStatus::Filled) {
            Qty(0)
        } else {
            Qty(1_000_000)
        },
        filled_qty: if matches!(status, OrderStatus::Filled) {
            Qty(1_000_000)
        } else {
            Qty(0)
        },
        avg_fill_px: Price(6_500_012),
        last_fill_px: Price(6_500_012),
        last_fill_qty: if matches!(status, OrderStatus::Filled) {
            Qty(1_000_000)
        } else {
            Qty(0)
        },
        sequence: 2,
        recv_ts_ns: 2_000,
        source: OrderEventSource::FixOrderEntry,
    }
}

fn field(message: &[u8], tag: u32) -> Option<&[u8]> {
    let prefix = format!("{tag}=");
    message
        .split(|b| *b == 1)
        .find_map(|raw| raw.strip_prefix(prefix.as_bytes()))
}

#[test]
fn runtime_pipeline_routes_market_to_strategy_to_order_thread() {
    let order = quoted_order();
    let strategy = QuoteOnFirstL1Strategy::new(order);
    let order_engine = OrderThreadEngine::new(
        FixEncoder::new("FIX.4.2", "SENDER", "TARGET"),
        OrderManager::with_risk(RiskConfig::default()),
        vec![btc_spec()],
    );
    let mut pipeline = RuntimePipeline::new(
        RuntimePipelineConfig::new(vec![btc_spec()], 8, 8).unwrap(),
        vec![Box::new(strategy)],
        order_engine,
    );

    let steps = pipeline.on_market_events(vec![l1_event()], "20260615-12:00:00.000");

    assert!(steps.contains(&RuntimePipelineStep::MarketEventRouted {
        symbol_id: SymbolId(0)
    }));
    assert!(steps.contains(&RuntimePipelineStep::StrategyCommandRouted {
        symbol_id: SymbolId(0)
    }));
    let send = steps
        .iter()
        .find_map(|step| match step {
            RuntimePipelineStep::OrderAction(OrderThreadAction::SendFix(bytes)) => Some(bytes),
            _ => None,
        })
        .expect("expected order thread to send FIX");
    assert_eq!(field(send, 35), Some(&b"D"[..]));
    assert_eq!(field(send, 11), Some(&b"cbhft-1"[..]));
    assert_eq!(
        pipeline.order_engine().manager().status("cbhft-1"),
        Some(OrderStatus::PendingNew)
    );
}

#[test]
fn runtime_pipeline_broadcasts_order_and_fill_events_back_to_strategy_threads() {
    let order_engine = OrderThreadEngine::new(
        FixEncoder::new("FIX.4.2", "SENDER", "TARGET"),
        OrderManager::with_risk(RiskConfig::default()),
        vec![btc_spec()],
    );
    let mut pipeline = RuntimePipeline::new(
        RuntimePipelineConfig::new(vec![btc_spec()], 8, 8).unwrap(),
        vec![Box::new(QuoteOnFirstL1Strategy::new(quoted_order()))],
        order_engine,
    );

    let events = vec![
        ExchangeEvent::Order(order_event(OrderStatus::Filled)),
        ExchangeEvent::Fill(FillEvent {
            symbol_id: SymbolId(0),
            client_order_id: "cbhft-1".to_string(),
            exchange_order_id: "ex-1".to_string(),
            exec_id: "exec-Filled".to_string(),
            side: Side::Buy,
            price: Price(6_500_012),
            qty: Qty(1_000_000),
            recv_ts_ns: 2_000,
        }),
    ];

    let steps = pipeline.on_exchange_events(events, "20260615-12:00:01.000");

    assert_eq!(
        steps
            .iter()
            .filter(|step| matches!(
                step,
                RuntimePipelineStep::ExchangeEventRouted {
                    symbol_id: Some(SymbolId(0))
                }
            ))
            .count(),
        2
    );
    assert!(
        steps
            .iter()
            .all(|step| !matches!(step, RuntimePipelineStep::OrderAction(_)))
    );
}
