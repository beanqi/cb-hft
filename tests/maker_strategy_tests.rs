use cb_hft::event::{ExchangeEvent, FillEvent};
use cb_hft::maker::{MakerConfig, MakerStrategy, Trend, TrendConfig};
use cb_hft::market::{MarketEngine, MarketEvent, Trade};
use cb_hft::order::{OrderEvent, OrderEventSource, OrderStatus, StrategyCommand, TimeInForce};
use cb_hft::types::{Price, Qty, Side, SymbolId};

fn l1(seq: u64, bid_px: i64, bid_qty: i64, ask_px: i64, ask_qty: i64) -> MarketEvent {
    MarketEvent::L1 {
        symbol_id: SymbolId(0),
        recv_ts_ns: seq * 1_000_000,
        bid_px: Price(bid_px),
        bid_qty: Qty(bid_qty),
        ask_px: Price(ask_px),
        ask_qty: Qty(ask_qty),
        sequence: seq,
    }
}

fn trade(seq: u64, side: Side, px: i64, qty: i64) -> MarketEvent {
    MarketEvent::Trade(Trade {
        symbol_id: SymbolId(0),
        recv_ts_ns: seq * 1_000_000,
        trade_id: seq,
        side: Some(side),
        price: Price(px),
        qty: Qty(qty),
        sequence: seq,
    })
}

fn order_open(cid: &str, side: Side, px: i64) -> ExchangeEvent {
    ExchangeEvent::Order(OrderEvent {
        symbol_id: SymbolId(0),
        client_order_id: cid.to_string(),
        exchange_order_id: format!("ex-{cid}"),
        exec_id: format!("open-{cid}"),
        status: OrderStatus::Open,
        side,
        price: Price(px),
        original_qty: Qty(100),
        remaining_qty: Qty(100),
        filled_qty: Qty(0),
        avg_fill_px: Price(0),
        last_fill_px: Price(0),
        last_fill_qty: Qty(0),
        sequence: 1,
        recv_ts_ns: 10_000_000,
        source: OrderEventSource::FixOrderEntry,
    })
}

fn fill(cid: &str, side: Side, px: i64) -> ExchangeEvent {
    ExchangeEvent::Fill(FillEvent {
        symbol_id: SymbolId(0),
        client_order_id: cid.to_string(),
        exchange_order_id: format!("ex-{cid}"),
        exec_id: format!("fill-{cid}"),
        side,
        price: Price(px),
        qty: Qty(100),
        recv_ts_ns: 20_000_000,
    })
}

fn strategy() -> MakerStrategy {
    MakerStrategy::new(
        MakerConfig {
            symbol_id: SymbolId(0),
            quote_qty: Qty(100),
            quote_tick_offset: 0,
            requote_ticks: 1,
        },
        TrendConfig {
            window_ns: 2_000_000_000,
            min_window_notional: 5_000_000,
            strong_score_x100: 150,
            trade_weight_x100: 100,
            count_weight_x100: 50,
            obi_weight_x100: 80,
            micro_weight_x100: 40,
        },
    )
}

#[test]
fn trend_guard_classifies_neutral_and_strong_trade_pressure() {
    let mut s = strategy();
    assert_eq!(s.trend(), Trend::Neutral);

    s.on_trade_sample(&Trade {
        symbol_id: SymbolId(0),
        recv_ts_ns: 1,
        trade_id: 1,
        side: Some(Side::Buy),
        price: Price(50_000_00),
        qty: Qty(100_000_000),
        sequence: 1,
    });
    s.on_book_sample(Price(50_000_00), Qty(100), Price(50_001_00), Qty(100), 2);

    assert_eq!(s.trend(), Trend::StrongUp);
}

#[test]
fn maker_quotes_two_sides_when_trend_is_neutral() {
    let mut engine = MarketEngine::new(SymbolId(0), strategy());

    let emitted = engine.on_market_event(l1(1, 10_000, 1_000, 10_010, 1_000));

    assert_eq!(emitted.len(), 2);
    assert!(
        matches!(emitted[0], StrategyCommand::NewOrder(cmd) if cmd.side == Side::Buy && cmd.price == Price(10_000) && cmd.post_only && cmd.time_in_force == TimeInForce::GoodTillCancel)
    );
    assert!(
        matches!(emitted[1], StrategyCommand::NewOrder(cmd) if cmd.side == Side::Sell && cmd.price == Price(10_010) && cmd.post_only && cmd.time_in_force == TimeInForce::GoodTillCancel)
    );
}

#[test]
fn maker_cancels_quotes_and_stays_flat_when_trend_is_strong() {
    let mut engine = MarketEngine::new(SymbolId(0), strategy());
    assert_eq!(
        engine
            .on_market_event(l1(1, 10_000, 1_000, 10_010, 1_000))
            .len(),
        2
    );

    for seq in 2..7 {
        engine.on_market_event(trade(seq, Side::Buy, 10_010, 1_000));
    }
    let emitted = engine.on_market_event(l1(8, 10_010, 1_000, 10_020, 1_000));

    assert_eq!(
        emitted,
        vec![StrategyCommand::CancelAll {
            symbol_id: SymbolId(0),
            signal_ts_ns: 8_000_000
        }]
    );
}

#[test]
fn maker_chases_opposite_side_after_one_quote_fills() {
    let mut engine = MarketEngine::new(SymbolId(0), strategy());
    assert_eq!(
        engine
            .on_market_event(l1(1, 10_000, 1_000, 10_010, 1_000))
            .len(),
        2
    );
    engine.on_exchange_event(order_open("cbhft-1", Side::Buy, 10_000));
    engine.on_exchange_event(order_open("cbhft-2", Side::Sell, 10_010));

    let emitted = engine.on_exchange_event(fill("cbhft-1", Side::Buy, 10_000));

    assert_eq!(emitted.len(), 2);
    assert!(matches!(emitted[0], StrategyCommand::CancelOrder(cmd) if cmd.client_order_id == 2));
    assert!(
        matches!(emitted[1], StrategyCommand::NewOrder(cmd) if cmd.side == Side::Sell && cmd.price == Price(10_010))
    );

    engine.on_exchange_event(order_open("cbhft-3", Side::Sell, 10_010));
    let emitted = engine.on_market_event(l1(2, 10_005, 1_000, 10_015, 1_000));
    assert_eq!(emitted.len(), 2);
    assert!(matches!(emitted[0], StrategyCommand::CancelOrder(cmd) if cmd.client_order_id == 3));
    assert!(
        matches!(emitted[1], StrategyCommand::NewOrder(cmd) if cmd.side == Side::Sell && cmd.price == Price(10_015))
    );

    let emitted = engine.on_exchange_event(fill("cbhft-4", Side::Sell, 10_015));
    assert!(emitted.is_empty());
    let emitted = engine.on_market_event(l1(3, 10_000, 1_000, 10_010, 1_000));
    assert_eq!(emitted.len(), 2);
}
