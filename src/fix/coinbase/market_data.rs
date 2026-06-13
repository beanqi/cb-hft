use crate::fix::{FixFrame, FixParser};
use crate::market::{MarketEvent, Trade};
use crate::types::{DecimalParseError, Price, ProductSpec, Qty};

#[derive(Debug, PartialEq, Eq)]
pub enum MarketDataError {
    Decimal(DecimalParseError),
    InvalidUnsignedInteger,
}

impl From<DecimalParseError> for MarketDataError {
    fn from(value: DecimalParseError) -> Self {
        Self::Decimal(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryType {
    Bid,
    Offer,
    Trade,
    Other,
}

#[derive(Default)]
struct PendingEntry {
    entry_type: Option<EntryType>,
    price: Option<Price>,
    qty: Option<Qty>,
    trade_id: Option<u64>,
}

pub fn parse_market_data(
    parser: &FixParser,
    frame: &FixFrame<'_>,
    spec: &ProductSpec,
    recv_ts_ns: u64,
) -> Result<Vec<MarketEvent>, MarketDataError> {
    let mut sequence = 0u64;
    let mut events = Vec::new();
    let mut bid: Option<(Price, Qty)> = None;
    let mut ask: Option<(Price, Qty)> = None;
    let mut pending = PendingEntry::default();

    for field in parser.fields(frame) {
        match field.tag {
            34 => sequence = parse_u64(field.value)?,
            269 => {
                flush_entry(
                    &mut pending,
                    spec,
                    recv_ts_ns,
                    sequence,
                    &mut bid,
                    &mut ask,
                    &mut events,
                );
                pending.entry_type = Some(match field.value {
                    b"0" => EntryType::Bid,
                    b"1" => EntryType::Offer,
                    b"2" => EntryType::Trade,
                    _ => EntryType::Other,
                });
            }
            270 => pending.price = Some(Price::parse_scaled(field.value, spec.price_scale)?),
            271 => pending.qty = Some(Qty::parse_scaled(field.value, spec.qty_scale)?),
            278 => pending.trade_id = Some(parse_u64(field.value)?),
            _ => {}
        }
    }

    flush_entry(
        &mut pending,
        spec,
        recv_ts_ns,
        sequence,
        &mut bid,
        &mut ask,
        &mut events,
    );

    if let (Some((bid_px, bid_qty)), Some((ask_px, ask_qty))) = (bid, ask) {
        events.insert(
            0,
            MarketEvent::L1 {
                symbol_id: spec.symbol_id,
                recv_ts_ns,
                bid_px,
                bid_qty,
                ask_px,
                ask_qty,
                sequence,
            },
        );
    }

    Ok(events)
}

fn flush_entry(
    pending: &mut PendingEntry,
    spec: &ProductSpec,
    recv_ts_ns: u64,
    sequence: u64,
    bid: &mut Option<(Price, Qty)>,
    ask: &mut Option<(Price, Qty)>,
    events: &mut Vec<MarketEvent>,
) {
    match (pending.entry_type, pending.price, pending.qty) {
        (Some(EntryType::Bid), Some(px), Some(qty)) => *bid = Some((px, qty)),
        (Some(EntryType::Offer), Some(px), Some(qty)) => *ask = Some((px, qty)),
        (Some(EntryType::Trade), Some(price), Some(qty)) => {
            events.push(MarketEvent::Trade(Trade {
                symbol_id: spec.symbol_id,
                recv_ts_ns,
                trade_id: pending.trade_id.unwrap_or(0),
                price,
                qty,
                sequence,
            }));
        }
        _ => {}
    }
    *pending = PendingEntry::default();
}

fn parse_u64(input: &[u8]) -> Result<u64, MarketDataError> {
    if input.is_empty() {
        return Err(MarketDataError::InvalidUnsignedInteger);
    }
    let mut value = 0u64;
    for &b in input {
        if !b.is_ascii_digit() {
            return Err(MarketDataError::InvalidUnsignedInteger);
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add((b - b'0') as u64))
            .ok_or(MarketDataError::InvalidUnsignedInteger)?;
    }
    Ok(value)
}
