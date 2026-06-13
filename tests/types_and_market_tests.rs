use cb_hft::market::{L1Book, L1Update};
use cb_hft::order::{NewOrderCommand, StrategyCommand, TimeInForce};
use cb_hft::ring::{CommandRings, RingError};
use cb_hft::types::{Price, ProductSpec, Qty, Side, SymbolId};

#[test]
fn parses_decimal_price_and_quantity_to_scaled_integers() {
    assert_eq!(Price::parse_scaled(b"123.45", 100).unwrap(), Price(12_345));
    assert_eq!(Price::parse_scaled(b"123", 100).unwrap(), Price(12_300));
    assert_eq!(
        Qty::parse_scaled(b"0.00000042", 100_000_000).unwrap(),
        Qty(42)
    );
    assert_eq!(
        Qty::parse_scaled(b"1.2", 100_000_000).unwrap(),
        Qty(120_000_000)
    );
}

#[test]
fn rejects_decimal_values_that_exceed_configured_scale() {
    assert!(Price::parse_scaled(b"1.001", 100).is_err());
    assert!(Qty::parse_scaled(b"0.000000001", 100_000_000).is_err());
    assert!(Price::parse_scaled(b"", 100).is_err());
    assert!(Price::parse_scaled(b"12x.3", 100).is_err());
}

#[test]
fn l1_book_applies_only_strictly_newer_sequence_numbers() {
    let symbol_id = SymbolId(1);
    let mut book = L1Book::default();

    book.apply(L1Update {
        symbol_id,
        exchange_ts_ns: 10,
        recv_ts_ns: 11,
        bid_px: Price(100),
        bid_qty: Qty(5),
        ask_px: Price(101),
        ask_qty: Qty(6),
        sequence: 7,
    });

    book.apply(L1Update {
        symbol_id,
        exchange_ts_ns: 12,
        recv_ts_ns: 13,
        bid_px: Price(90),
        bid_qty: Qty(1),
        ask_px: Price(91),
        ask_qty: Qty(1),
        sequence: 7,
    });

    assert_eq!(book.bid_px, Price(100));
    assert_eq!(book.ask_px, Price(101));
    assert_eq!(book.last_sequence, 7);

    book.apply(L1Update {
        symbol_id,
        exchange_ts_ns: 14,
        recv_ts_ns: 15,
        bid_px: Price(102),
        bid_qty: Qty(7),
        ask_px: Price(103),
        ask_qty: Qty(8),
        sequence: 8,
    });

    assert_eq!(book.bid_px, Price(102));
    assert_eq!(book.bid_qty, Qty(7));
    assert_eq!(book.ask_px, Price(103));
    assert_eq!(book.ask_qty, Qty(8));
    assert_eq!(book.last_sequence, 8);
    assert_eq!(book.last_update_recv_ns, 15);
}

#[test]
fn command_ring_preserves_strategy_command_order() {
    let mut rings = CommandRings::new(2, 2).unwrap();

    rings
        .producer_mut(SymbolId(0))
        .unwrap()
        .push(StrategyCommand::NewOrder(NewOrderCommand {
            symbol_id: SymbolId(0),
            side: Side::Buy,
            price: Price(10_000),
            qty: Qty(100),
            post_only: true,
            time_in_force: TimeInForce::GoodTillCancel,
            strategy_order_id: 1,
            signal_ts_ns: 123,
        }))
        .unwrap();

    rings
        .producer_mut(SymbolId(0))
        .unwrap()
        .push(StrategyCommand::NewOrder(NewOrderCommand {
            symbol_id: SymbolId(0),
            side: Side::Sell,
            price: Price(10_100),
            qty: Qty(200),
            post_only: true,
            time_in_force: TimeInForce::GoodTillCancel,
            strategy_order_id: 2,
            signal_ts_ns: 124,
        }))
        .unwrap();

    assert_eq!(
        rings
            .producer_mut(SymbolId(0))
            .unwrap()
            .push(StrategyCommand::CancelAll {
                symbol_id: SymbolId(0),
                signal_ts_ns: 125
            }),
        Err(RingError::Full)
    );

    let first = rings.consumer_mut(SymbolId(0)).unwrap().pop().unwrap();
    let second = rings.consumer_mut(SymbolId(0)).unwrap().pop().unwrap();

    assert_eq!(first.strategy_order_id(), 1);
    assert_eq!(second.strategy_order_id(), 2);
    assert!(rings.consumer_mut(SymbolId(0)).unwrap().pop().is_none());
}

#[test]
fn product_spec_validates_tick_and_size_steps() {
    let spec = ProductSpec {
        symbol_id: SymbolId(0),
        coinbase_product: "BTC-USD",
        price_scale: 100,
        qty_scale: 100_000_000,
        min_qty: Qty(10),
        min_notional: 100,
        price_tick: Price(1),
        qty_step: Qty(10),
    };

    assert!(spec.validate_order(Price(10_000), Qty(20)).is_ok());
    assert!(spec.validate_order(Price(10_000), Qty(11)).is_err());
    assert!(spec.validate_order(Price(10_000), Qty(1)).is_err());
}
