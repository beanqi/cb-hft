use cb_hft::event::{BalanceEvent, ExchangeEvent, FillEvent};
use cb_hft::order::{OrderEvent, OrderEventSource, OrderStatus};
use cb_hft::ring::{EventRingPair, RingError};
use cb_hft::types::{AssetId, Price, Qty, Side, SymbolId};

fn order_event() -> OrderEvent {
    OrderEvent {
        symbol_id: SymbolId(0),
        client_order_id: "cid-1".to_string(),
        exchange_order_id: "ex-1".to_string(),
        exec_id: "exec-1".to_string(),
        status: OrderStatus::Open,
        side: Side::Buy,
        price: Price(100),
        original_qty: Qty(10),
        remaining_qty: Qty(10),
        filled_qty: Qty(0),
        avg_fill_px: Price(0),
        last_fill_px: Price(0),
        last_fill_qty: Qty(0),
        sequence: 1,
        recv_ts_ns: 10,
        source: OrderEventSource::FixOrderEntry,
    }
}

#[test]
fn exchange_event_reports_its_symbol_when_symbol_scoped() {
    let order = ExchangeEvent::Order(order_event());
    assert_eq!(order.symbol_id(), Some(SymbolId(0)));

    let fill = ExchangeEvent::Fill(FillEvent {
        symbol_id: SymbolId(1),
        client_order_id: "cid-2".to_string(),
        exchange_order_id: "ex-2".to_string(),
        exec_id: "exec-2".to_string(),
        side: Side::Sell,
        price: Price(200),
        qty: Qty(5),
        recv_ts_ns: 20,
    });
    assert_eq!(fill.symbol_id(), Some(SymbolId(1)));

    let balance = ExchangeEvent::Balance(BalanceEvent {
        asset_id: AssetId::from_static("USD"),
        total: Qty(1_000),
        available: Qty(900),
        hold: Qty(100),
        update_ts_ns: 30,
        recv_ts_ns: 31,
    });
    assert_eq!(balance.symbol_id(), None);
}

#[test]
fn event_ring_pair_preserves_exchange_event_order() {
    let (mut producer, mut consumer) = EventRingPair::new(2).unwrap();

    producer.push(ExchangeEvent::Order(order_event())).unwrap();
    producer
        .push(ExchangeEvent::Balance(BalanceEvent {
            asset_id: AssetId::from_static("BTC"),
            total: Qty(10),
            available: Qty(8),
            hold: Qty(2),
            update_ts_ns: 50,
            recv_ts_ns: 51,
        }))
        .unwrap();

    assert_eq!(
        producer.push(ExchangeEvent::Order(order_event())),
        Err(RingError::Full)
    );
    assert!(matches!(consumer.pop(), Some(ExchangeEvent::Order(_))));
    assert!(matches!(consumer.pop(), Some(ExchangeEvent::Balance(_))));
    assert_eq!(consumer.pop(), None);
}

#[test]
fn event_ring_pair_rejects_zero_capacity() {
    assert_eq!(EventRingPair::new(0).err(), Some(RingError::ZeroCapacity));
}
