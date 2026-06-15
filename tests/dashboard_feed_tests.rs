use cb_hft::dashboard_feed::{DashboardFeed, FeedEvent};
use cb_hft::event::{ExchangeEvent, FillEvent};
use cb_hft::market::MarketEvent;
use cb_hft::order::{OrderEvent, OrderEventSource, OrderStatus};
use cb_hft::types::{Price, Qty, Side, SymbolId};

#[test]
fn dashboard_feed_keeps_latest_l1_trades_orders_and_fills() {
    let feed = DashboardFeed::new(2);

    feed.publish_market(MarketEvent::L1 {
        symbol_id: SymbolId(0),
        recv_ts_ns: 1,
        bid_px: Price(100),
        bid_qty: Qty(10),
        ask_px: Price(101),
        ask_qty: Qty(11),
        sequence: 9,
    });
    feed.publish_market(MarketEvent::Trade(cb_hft::market::Trade {
        symbol_id: SymbolId(0),
        recv_ts_ns: 2,
        trade_id: 7,
        side: Some(Side::Buy),
        price: Price(101),
        qty: Qty(5),
        sequence: 10,
    }));
    feed.publish_exchange(ExchangeEvent::Order(order_event("cbhft-1")));
    feed.publish_exchange(ExchangeEvent::Fill(FillEvent {
        symbol_id: SymbolId(0),
        client_order_id: "cbhft-1".to_string(),
        exchange_order_id: "ex-1".to_string(),
        exec_id: "fill-1".to_string(),
        side: Side::Sell,
        price: Price(100),
        qty: Qty(3),
        recv_ts_ns: 4,
    }));

    let snapshot = feed.snapshot();
    assert_eq!(snapshot.l1[0].bid_px, 100);
    assert_eq!(snapshot.trades[0].trade_id, 7);
    assert_eq!(snapshot.orders[0].client_order_id, "cbhft-1");
    assert_eq!(snapshot.fills[0].exec_id, "fill-1");

    let events = feed.events_after(0);
    assert_eq!(events.len(), 4);
    assert!(matches!(events[0].event, FeedEvent::L1(_)));
}

fn order_event(client_order_id: &str) -> OrderEvent {
    OrderEvent {
        symbol_id: SymbolId(0),
        client_order_id: client_order_id.to_string(),
        exchange_order_id: "ex-1".to_string(),
        exec_id: "open-1".to_string(),
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
        recv_ts_ns: 3,
        source: OrderEventSource::FixOrderEntry,
    }
}
