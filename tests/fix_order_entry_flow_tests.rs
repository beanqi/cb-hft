use cb_hft::event::{ExchangeEvent, FillEvent};
use cb_hft::fix::{FixEncoder, FixParser, MsgType};
use cb_hft::order::{
    NewOrderCommand, OrderEventSource, OrderManager, OrderStatus, OrderThreadAction,
    OrderThreadEngine, RiskConfig, StrategyCommand, TimeInForce,
};
use cb_hft::runtime::RuntimeOptions;
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

fn new_order() -> StrategyCommand {
    StrategyCommand::NewOrder(NewOrderCommand {
        symbol_id: SymbolId(0),
        side: Side::Buy,
        price: Price(6_500_012),
        qty: Qty(1_000_000),
        post_only: true,
        time_in_force: TimeInForce::GoodTillCancel,
        strategy_order_id: 42,
        signal_ts_ns: 1_000,
    })
}

fn field(message: &[u8], tag: u32) -> Option<&[u8]> {
    let prefix = format!("{tag}=");
    message
        .split(|b| *b == 1)
        .find_map(|raw| raw.strip_prefix(prefix.as_bytes()))
}

fn fix_message(body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"8=FIX.4.2\x01");
    msg.extend_from_slice(format!("9={}\x01", body.len()).as_bytes());
    msg.extend_from_slice(body);
    let checksum = msg.iter().fold(0u32, |acc, b| acc + *b as u32) % 256;
    msg.extend_from_slice(format!("10={checksum:03}\x01").as_bytes());
    msg
}

#[test]
fn order_thread_round_trips_command_fix_execution_report_to_order_and_fill_events() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let manager = OrderManager::with_risk(RiskConfig::default());
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![btc_spec()]);

    let actions = engine.on_command(new_order(), "20260613-12:00:00.000");
    let OrderThreadAction::SendFix(order_fix) = &actions[0] else {
        panic!("expected FIX send: {actions:?}");
    };
    assert_eq!(field(order_fix, 35), Some(&b"D"[..]));
    assert_eq!(field(order_fix, 11), Some(&b"cbhft-1"[..]));

    let parser = FixParser::default();
    let msg = fix_message(
        b"35=8\x0134=2\x01150=F\x0139=2\x0111=cbhft-1\x0137=ex-1\x0117=exec-fill-1\x0155=BTC-USD\x0154=1\x0144=65000.12\x0138=0.01000000\x01151=0\x0114=0.01000000\x016=65000.12\x0131=65000.12\x0132=0.01000000\x01",
    );
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();
    assert_eq!(frame.msg_type, MsgType::ExecutionReport);

    let events = engine
        .on_execution_report(&parser, &frame, 2_000)
        .expect("execution report should parse");

    assert_eq!(
        engine.manager().status("cbhft-1"),
        Some(OrderStatus::Filled)
    );
    assert_eq!(events.len(), 2);
    match &events[0] {
        ExchangeEvent::Order(order) => {
            assert_eq!(order.client_order_id, "cbhft-1");
            assert_eq!(order.status, OrderStatus::Filled);
            assert_eq!(order.source, OrderEventSource::FixOrderEntry);
            assert_eq!(order.filled_qty, Qty(1_000_000));
        }
        other => panic!("expected order event, got {other:?}"),
    }
    assert_eq!(
        events[1],
        ExchangeEvent::Fill(FillEvent {
            symbol_id: SymbolId(0),
            client_order_id: "cbhft-1".to_string(),
            exchange_order_id: "ex-1".to_string(),
            exec_id: "exec-fill-1".to_string(),
            side: Side::Buy,
            price: Price(6_500_012),
            qty: Qty(1_000_000),
            recv_ts_ns: 2_000,
        })
    );

    let duplicate_events = engine
        .on_execution_report(&parser, &frame, 3_000)
        .expect("duplicate execution report should parse");
    assert!(duplicate_events.is_empty());
}

#[test]
fn runtime_options_can_enable_order_entry_without_market_data() {
    let opts = RuntimeOptions::parse_args([
        "--config",
        "config/sandbox.toml.example",
        "--order-only",
        "--once",
    ])
    .unwrap();

    assert!(opts.order_entry);
    assert!(!opts.market_data);
    assert!(!opts.account);
    assert!(opts.once);
}
