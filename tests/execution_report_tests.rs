use cb_hft::fix::FixParser;
use cb_hft::fix::coinbase::order_entry::parse_execution_report;
use cb_hft::order::{OrderEvent, OrderEventSource, OrderManager, OrderStatus};
use cb_hft::types::{Price, ProductSpec, Qty, Side, SymbolId};

fn fix_message(body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"8=FIX.4.2\x01");
    msg.extend_from_slice(format!("9={}\x01", body.len()).as_bytes());
    msg.extend_from_slice(body);
    let checksum = msg.iter().fold(0u32, |acc, b| acc + *b as u32) % 256;
    msg.extend_from_slice(format!("10={checksum:03}\x01").as_bytes());
    msg
}

fn btc_spec() -> ProductSpec {
    ProductSpec {
        symbol_id: SymbolId(0),
        coinbase_product: "BTC-USD",
        price_scale: 100,
        qty_scale: 100_000_000,
        min_qty: Qty(1),
        min_notional: 1,
        price_tick: Price(1),
        qty_step: Qty(1),
    }
}

#[test]
fn parses_new_order_execution_report_into_order_event() {
    let parser = FixParser::default();
    let msg = fix_message(
        b"35=8\x0134=10\x01150=0\x0139=0\x0111=cid-1\x0137=ex-1\x0117=exec-1\x0155=BTC-USD\x0154=1\x0144=65000.12\x0138=0.01\x01151=0.01\x0114=0\x016=0\x01",
    );
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let event = parse_execution_report(&parser, &frame, &btc_spec(), 1_000).unwrap();

    assert_eq!(
        event,
        OrderEvent {
            symbol_id: SymbolId(0),
            client_order_id: "cid-1".to_string(),
            exchange_order_id: "ex-1".to_string(),
            exec_id: "exec-1".to_string(),
            status: OrderStatus::Open,
            side: Side::Buy,
            price: Price(6_500_012),
            original_qty: Qty(1_000_000),
            remaining_qty: Qty(1_000_000),
            filled_qty: Qty(0),
            avg_fill_px: Price(0),
            last_fill_px: Price(0),
            last_fill_qty: Qty(0),
            sequence: 10,
            recv_ts_ns: 1_000,
            source: OrderEventSource::FixOrderEntry,
        }
    );
}

#[test]
fn parses_partial_fill_execution_report() {
    let parser = FixParser::default();
    let msg = fix_message(
        b"35=8\x0134=11\x01150=F\x0139=1\x0111=cid-1\x0137=ex-1\x0117=exec-2\x0155=BTC-USD\x0154=2\x0144=65000.12\x0138=0.03\x01151=0.02\x0114=0.01\x016=65000.12\x0131=65000.12\x0132=0.01\x01",
    );
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let event = parse_execution_report(&parser, &frame, &btc_spec(), 2_000).unwrap();

    assert_eq!(event.status, OrderStatus::PartiallyFilled);
    assert_eq!(event.side, Side::Sell);
    assert_eq!(event.original_qty, Qty(3_000_000));
    assert_eq!(event.remaining_qty, Qty(2_000_000));
    assert_eq!(event.filled_qty, Qty(1_000_000));
    assert_eq!(event.avg_fill_px, Price(6_500_012));
    assert_eq!(event.last_fill_px, Price(6_500_012));
    assert_eq!(event.last_fill_qty, Qty(1_000_000));
}

#[test]
fn order_manager_applies_each_exec_id_once_and_tracks_latest_status() {
    let mut manager = OrderManager::default();
    let event = OrderEvent {
        symbol_id: SymbolId(0),
        client_order_id: "cid-1".to_string(),
        exchange_order_id: "ex-1".to_string(),
        exec_id: "exec-1".to_string(),
        status: OrderStatus::Open,
        side: Side::Buy,
        price: Price(1),
        original_qty: Qty(10),
        remaining_qty: Qty(10),
        filled_qty: Qty(0),
        avg_fill_px: Price(0),
        last_fill_px: Price(0),
        last_fill_qty: Qty(0),
        sequence: 1,
        recv_ts_ns: 1,
        source: OrderEventSource::FixOrderEntry,
    };

    assert!(manager.apply_order_event(event.clone()));
    assert!(!manager.apply_order_event(event.clone()));
    assert_eq!(manager.status("cid-1"), Some(OrderStatus::Open));

    let mut filled = event;
    filled.exec_id = "exec-2".to_string();
    filled.status = OrderStatus::Filled;
    filled.remaining_qty = Qty(0);
    filled.filled_qty = Qty(10);

    assert!(manager.apply_order_event(filled));
    assert_eq!(manager.status("cid-1"), Some(OrderStatus::Filled));
}

#[test]
fn maps_rejected_and_canceled_order_statuses() {
    let parser = FixParser::default();
    let rejected = fix_message(
        b"35=8\x0134=12\x01150=8\x0139=8\x0111=cid-r\x0137=ex-r\x0117=exec-r\x0155=BTC-USD\x0154=1\x0144=1\x0138=1\x01151=1\x0114=0\x016=0\x01",
    );
    let canceled = fix_message(
        b"35=8\x0134=13\x01150=4\x0139=4\x0111=cid-c\x0137=ex-c\x0117=exec-c\x0155=BTC-USD\x0154=1\x0144=1\x0138=1\x01151=0\x0114=0\x016=0\x01",
    );

    let (rejected_frame, _) = parser.next_frame(&rejected).unwrap().unwrap();
    let (canceled_frame, _) = parser.next_frame(&canceled).unwrap().unwrap();

    assert_eq!(
        parse_execution_report(&parser, &rejected_frame, &btc_spec(), 1)
            .unwrap()
            .status,
        OrderStatus::Rejected
    );
    assert_eq!(
        parse_execution_report(&parser, &canceled_frame, &btc_spec(), 1)
            .unwrap()
            .status,
        OrderStatus::Canceled
    );
}
