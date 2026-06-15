use crate::fix::{FixFrame, FixParser};
use crate::order::{OrderEvent, OrderEventSource, OrderStatus};
use crate::types::{DecimalParseError, Price, ProductSpec, Qty, Side};

#[derive(Debug, PartialEq, Eq)]
pub enum OrderEntryError {
    Decimal(DecimalParseError),
    InvalidUnsignedInteger,
    MissingClientOrderId,
    MissingExecId,
    MissingSymbol,
    UnknownSymbol,
    InvalidSide,
}

impl From<DecimalParseError> for OrderEntryError {
    fn from(value: DecimalParseError) -> Self {
        Self::Decimal(value)
    }
}

#[derive(Default)]
struct ExecutionReportFields {
    sequence: u64,
    exec_type: Option<Vec<u8>>,
    ord_status: Option<Vec<u8>>,
    client_order_id: Option<String>,
    exchange_order_id: Option<String>,
    exec_id: Option<String>,
    side: Option<Side>,
    price: Price,
    original_qty: Qty,
    remaining_qty: Qty,
    filled_qty: Qty,
    avg_fill_px: Price,
    last_fill_px: Price,
    last_fill_qty: Qty,
}

pub fn parse_execution_report(
    parser: &FixParser,
    frame: &FixFrame<'_>,
    spec: &ProductSpec,
    recv_ts_ns: u64,
) -> Result<OrderEvent, OrderEntryError> {
    let mut fields = ExecutionReportFields::default();

    for field in parser.fields(frame) {
        match field.tag {
            34 => fields.sequence = parse_u64(field.value)?,
            150 => fields.exec_type = Some(field.value.to_vec()),
            39 => fields.ord_status = Some(field.value.to_vec()),
            11 => fields.client_order_id = Some(bytes_to_string(field.value)),
            37 => fields.exchange_order_id = Some(bytes_to_string(field.value)),
            17 => fields.exec_id = Some(bytes_to_string(field.value)),
            54 => fields.side = Some(parse_side(field.value)?),
            44 => fields.price = Price::parse_scaled(field.value, spec.price_scale)?,
            38 => fields.original_qty = Qty::parse_scaled(field.value, spec.qty_scale)?,
            151 => fields.remaining_qty = Qty::parse_scaled(field.value, spec.qty_scale)?,
            14 => fields.filled_qty = Qty::parse_scaled(field.value, spec.qty_scale)?,
            6 => fields.avg_fill_px = Price::parse_scaled(field.value, spec.price_scale)?,
            31 => fields.last_fill_px = Price::parse_scaled(field.value, spec.price_scale)?,
            32 => fields.last_fill_qty = Qty::parse_scaled(field.value, spec.qty_scale)?,
            _ => {}
        }
    }

    let status = parse_status(fields.ord_status.as_deref(), fields.exec_type.as_deref());

    Ok(OrderEvent {
        symbol_id: spec.symbol_id,
        client_order_id: fields
            .client_order_id
            .ok_or(OrderEntryError::MissingClientOrderId)?,
        exchange_order_id: fields.exchange_order_id.unwrap_or_default(),
        exec_id: fields.exec_id.ok_or(OrderEntryError::MissingExecId)?,
        status,
        side: fields.side.ok_or(OrderEntryError::InvalidSide)?,
        price: fields.price,
        original_qty: fields.original_qty,
        remaining_qty: fields.remaining_qty,
        filled_qty: fields.filled_qty,
        avg_fill_px: fields.avg_fill_px,
        last_fill_px: fields.last_fill_px,
        last_fill_qty: fields.last_fill_qty,
        sequence: fields.sequence,
        recv_ts_ns,
        source: OrderEventSource::FixOrderEntry,
    })
}

fn parse_status(ord_status: Option<&[u8]>, exec_type: Option<&[u8]>) -> OrderStatus {
    let value = ord_status.or(exec_type).unwrap_or_default();
    match value {
        b"A" => OrderStatus::PendingNew,
        b"0" => OrderStatus::Open,
        b"1" => OrderStatus::PartiallyFilled,
        b"2" => OrderStatus::Filled,
        b"6" => OrderStatus::PendingCancel,
        b"4" => OrderStatus::Canceled,
        b"8" => OrderStatus::Rejected,
        _ => OrderStatus::Unknown,
    }
}

fn parse_side(input: &[u8]) -> Result<Side, OrderEntryError> {
    match input {
        b"1" => Ok(Side::Buy),
        b"2" => Ok(Side::Sell),
        _ => Err(OrderEntryError::InvalidSide),
    }
}

fn parse_u64(input: &[u8]) -> Result<u64, OrderEntryError> {
    if input.is_empty() {
        return Err(OrderEntryError::InvalidUnsignedInteger);
    }
    let mut value = 0u64;
    for &b in input {
        if !b.is_ascii_digit() {
            return Err(OrderEntryError::InvalidUnsignedInteger);
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add((b - b'0') as u64))
            .ok_or(OrderEntryError::InvalidUnsignedInteger)?;
    }
    Ok(value)
}

fn bytes_to_string(input: &[u8]) -> String {
    String::from_utf8_lossy(input).into_owned()
}
