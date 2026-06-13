use cb_hft::fix::FixParser;
use cb_hft::fix::coinbase::market_data::parse_market_data;
use cb_hft::market::{MarketEvent, Trade};
use cb_hft::types::{Price, ProductSpec, Qty, SymbolId};

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
fn parses_l1_bid_ask_snapshot_into_single_market_event() {
    let parser = FixParser::default();
    let msg = fix_message(
        b"35=W\x0134=42\x0155=BTC-USD\x01268=2\x01269=0\x01270=65000.12\x01271=0.50\x01269=1\x01270=65001.34\x01271=0.40\x01",
    );
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let events = parse_market_data(&parser, &frame, &btc_spec(), 1_000).unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        MarketEvent::L1 {
            symbol_id: SymbolId(0),
            recv_ts_ns: 1_000,
            bid_px: Price(6_500_012),
            bid_qty: Qty(50_000_000),
            ask_px: Price(6_500_134),
            ask_qty: Qty(40_000_000),
            sequence: 42,
        }
    );
}

#[test]
fn parses_trade_entry_into_trade_event() {
    let parser = FixParser::default();
    let msg = fix_message(
        b"35=X\x0134=43\x0155=BTC-USD\x01268=1\x01269=2\x01270=65010.25\x01271=0.01\x01278=987654\x01",
    );
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let events = parse_market_data(&parser, &frame, &btc_spec(), 2_000).unwrap();

    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        MarketEvent::Trade(Trade {
            symbol_id: SymbolId(0),
            recv_ts_ns: 2_000,
            trade_id: 987654,
            price: Price(6_501_025),
            qty: Qty(1_000_000),
            sequence: 43,
        })
    );
}
