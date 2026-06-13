use cb_hft::event::ExchangeEvent;
use cb_hft::market::{L1Update, MarketEngine, MarketEvent};
use cb_hft::order::{
    NewOrderCommand, OrderEvent, OrderEventSource, OrderStatus, StrategyCommand, TimeInForce,
};
use cb_hft::strategy::{NoopStrategy, QuoteOnFirstL1Strategy, RecordingStrategy, Strategy};
use cb_hft::types::{Price, Qty, Side, SymbolId};

fn l1_event(sequence: u64) -> MarketEvent {
    MarketEvent::L1 {
        symbol_id: SymbolId(0),
        recv_ts_ns: 1_000 + sequence,
        bid_px: Price(10_000),
        bid_qty: Qty(100),
        ask_px: Price(10_010),
        ask_qty: Qty(200),
        sequence,
    }
}

fn order_event(status: OrderStatus) -> OrderEvent {
    OrderEvent {
        symbol_id: SymbolId(0),
        client_order_id: "cid-1".to_string(),
        exchange_order_id: "ex-1".to_string(),
        exec_id: format!("exec-{status:?}"),
        status,
        side: Side::Buy,
        price: Price(10_000),
        original_qty: Qty(100),
        remaining_qty: Qty(100),
        filled_qty: Qty(0),
        avg_fill_px: Price(0),
        last_fill_px: Price(0),
        last_fill_qty: Qty(0),
        sequence: 9,
        recv_ts_ns: 2_000,
        source: OrderEventSource::FixOrderEntry,
    }
}

#[test]
fn noop_strategy_consumes_l1_without_emitting_commands() {
    let mut engine = MarketEngine::new(SymbolId(0), NoopStrategy::default());

    let emitted = engine.on_market_event(l1_event(1));

    assert!(emitted.is_empty());
    assert_eq!(engine.book().bid_px, Price(10_000));
    assert_eq!(engine.book().ask_px, Price(10_010));
}

#[test]
fn market_engine_ignores_l1_for_other_symbols() {
    let mut engine = MarketEngine::new(SymbolId(0), RecordingStrategy::default());
    let event = MarketEvent::L1 {
        symbol_id: SymbolId(1),
        recv_ts_ns: 1,
        bid_px: Price(1),
        bid_qty: Qty(1),
        ask_px: Price(2),
        ask_qty: Qty(1),
        sequence: 1,
    };

    let emitted = engine.on_market_event(event);

    assert!(emitted.is_empty());
    assert_eq!(engine.book().last_sequence, 0);
    assert_eq!(engine.strategy().l1_count, 0);
}

#[test]
fn strategy_can_emit_new_order_command_from_l1_callback() {
    let command = StrategyCommand::NewOrder(NewOrderCommand {
        symbol_id: SymbolId(0),
        side: Side::Buy,
        price: Price(9_999),
        qty: Qty(10),
        post_only: true,
        time_in_force: TimeInForce::GoodTillCancel,
        strategy_order_id: 42,
        signal_ts_ns: 1_001,
    });
    let mut engine = MarketEngine::new(SymbolId(0), QuoteOnFirstL1Strategy::new(command));

    let emitted = engine.on_market_event(l1_event(1));

    assert_eq!(emitted, vec![command]);
    assert!(engine.on_market_event(l1_event(2)).is_empty());
}

#[test]
fn market_engine_routes_order_events_to_strategy_lifecycle() {
    let mut engine = MarketEngine::new(SymbolId(0), RecordingStrategy::default());

    engine.on_exchange_event(ExchangeEvent::Order(order_event(OrderStatus::Open)));
    engine.on_exchange_event(ExchangeEvent::Order(order_event(OrderStatus::Filled)));

    assert_eq!(
        engine.strategy().order_statuses,
        vec![OrderStatus::Open, OrderStatus::Filled]
    );
}

#[test]
fn strategy_context_can_emit_multiple_commands() {
    #[derive(Default)]
    struct TwoCommandStrategy;

    impl Strategy for TwoCommandStrategy {
        fn on_l1(
            &mut self,
            ctx: &mut cb_hft::strategy::StrategyContext<'_>,
            _book: &cb_hft::market::L1Book,
        ) {
            ctx.emit(StrategyCommand::CancelAll {
                symbol_id: ctx.symbol_id(),
                signal_ts_ns: ctx.now_ns(),
            });
            ctx.emit(StrategyCommand::CancelAll {
                symbol_id: ctx.symbol_id(),
                signal_ts_ns: ctx.now_ns() + 1,
            });
        }
    }

    let mut engine = MarketEngine::new(SymbolId(0), TwoCommandStrategy);
    let emitted = engine.on_market_event(l1_event(1));

    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[0].symbol_id(), SymbolId(0));
}

#[test]
fn l1_update_can_be_built_from_market_event() {
    let event = l1_event(7);
    let update = L1Update::try_from(event).unwrap();

    assert_eq!(update.sequence, 7);
    assert_eq!(update.bid_px, Price(10_000));
}
