use std::collections::HashMap;

use serde::Deserialize;

use crate::event::{BalanceEvent, ExchangeEvent, FillEvent};
use crate::order::{OrderEvent, OrderEventSource, OrderStatus};
use crate::types::{AssetId, Price, Qty, Side, SymbolId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSnapshot {
    balances: Vec<BalanceEvent>,
}

impl AccountSnapshot {
    pub fn new(balances: Vec<BalanceEvent>) -> Self {
        Self { balances }
    }

    pub fn balances(&self) -> &[BalanceEvent] {
        &self.balances
    }
}

#[derive(Default)]
pub struct BalanceBook {
    balances: HashMap<AssetId, BalanceEvent>,
}

impl BalanceBook {
    pub fn apply_snapshot(&mut self, snapshot: AccountSnapshot) {
        self.balances.clear();
        for balance in snapshot.balances {
            self.balances.insert(balance.asset_id.clone(), balance);
        }
    }

    pub fn apply_user_feed_event(&mut self, event: UserFeedEvent) {
        if let UserFeedEvent::Balance(balance) = event {
            self.balances.insert(balance.asset_id.clone(), balance);
        }
    }

    pub fn balance(&self, asset_id: &AssetId) -> Option<&BalanceEvent> {
        self.balances.get(asset_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserFeedEvent {
    Balance(BalanceEvent),
    Order(OrderEvent),
    Fill(FillEvent),
}

impl UserFeedEvent {
    pub fn into_exchange_event(self) -> ExchangeEvent {
        match self {
            Self::Balance(balance) => ExchangeEvent::Balance(balance),
            Self::Order(order) => ExchangeEvent::Order(order),
            Self::Fill(fill) => ExchangeEvent::Fill(fill),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountParseError {
    Json,
    MissingField,
    InvalidDecimal,
    UnsupportedMessageType,
}

impl core::fmt::Display for AccountParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for AccountParseError {}

pub fn parse_rest_accounts_snapshot(
    bytes: &[u8],
    qty_scale: i64,
    recv_ts_ns: u64,
) -> Result<AccountSnapshot, AccountParseError> {
    let accounts: Vec<RestAccount> =
        serde_json::from_slice(bytes).map_err(|_| AccountParseError::Json)?;
    let mut balances = Vec::with_capacity(accounts.len());
    for account in accounts {
        balances.push(BalanceEvent {
            asset_id: AssetId::new(account.currency),
            total: Qty::parse_scaled(account.balance.as_bytes(), qty_scale)
                .map_err(|_| AccountParseError::InvalidDecimal)?,
            available: Qty::parse_scaled(account.available.as_bytes(), qty_scale)
                .map_err(|_| AccountParseError::InvalidDecimal)?,
            hold: Qty::parse_scaled(account.hold.as_bytes(), qty_scale)
                .map_err(|_| AccountParseError::InvalidDecimal)?,
            update_ts_ns: 0,
            recv_ts_ns,
        });
    }
    Ok(AccountSnapshot::new(balances))
}

pub fn parse_user_feed_event_json(
    bytes: &[u8],
    sequence: u64,
    price_scale: i64,
    qty_scale: i64,
) -> Result<UserFeedEvent, AccountParseError> {
    let raw: RawUserFeedMessage =
        serde_json::from_slice(bytes).map_err(|_| AccountParseError::Json)?;
    match raw.message_type.as_str() {
        "balance" => Ok(UserFeedEvent::Balance(BalanceEvent {
            asset_id: AssetId::new(raw.currency.ok_or(AccountParseError::MissingField)?),
            total: parse_qty(raw.balance.as_deref(), qty_scale)?,
            available: parse_qty(raw.available.as_deref(), qty_scale)?,
            hold: parse_qty(raw.hold.as_deref(), qty_scale)?,
            update_ts_ns: sequence,
            recv_ts_ns: sequence,
        })),
        "open" => Ok(UserFeedEvent::Order(order_event(
            &raw,
            sequence,
            price_scale,
            qty_scale,
            OrderStatus::Open,
        )?)),
        "done" => {
            let status = if raw.reason.as_deref() == Some("filled") {
                OrderStatus::Filled
            } else {
                OrderStatus::Canceled
            };
            Ok(UserFeedEvent::Order(order_event(
                &raw,
                sequence,
                price_scale,
                qty_scale,
                status,
            )?))
        }
        "match" => Ok(UserFeedEvent::Fill(FillEvent {
            symbol_id: SymbolId(sequence as u16),
            client_order_id: raw.client_oid.unwrap_or_default(),
            exchange_order_id: raw
                .maker_order_id
                .or(raw.taker_order_id)
                .or(raw.order_id)
                .unwrap_or_default(),
            exec_id: raw
                .trade_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| sequence.to_string()),
            side: parse_side(raw.side.as_deref())?,
            price: parse_price(raw.price.as_deref(), price_scale)?,
            qty: parse_qty(raw.size.as_deref(), qty_scale)?,
            recv_ts_ns: sequence,
        })),
        _ => Err(AccountParseError::UnsupportedMessageType),
    }
}

fn order_event(
    raw: &RawUserFeedMessage,
    sequence: u64,
    price_scale: i64,
    qty_scale: i64,
    status: OrderStatus,
) -> Result<OrderEvent, AccountParseError> {
    let remaining_qty = parse_qty(
        raw.remaining_size.as_deref().or(raw.size.as_deref()),
        qty_scale,
    )?;
    Ok(OrderEvent {
        symbol_id: SymbolId(sequence as u16),
        client_order_id: raw.client_oid.clone().unwrap_or_default(),
        exchange_order_id: raw.order_id.clone().unwrap_or_default(),
        exec_id: raw.order_id.clone().unwrap_or_else(|| sequence.to_string()),
        status,
        side: parse_side(raw.side.as_deref())?,
        price: parse_price(raw.price.as_deref().or(Some("0")), price_scale)?,
        original_qty: remaining_qty,
        remaining_qty,
        filled_qty: Qty(0),
        avg_fill_px: Price(0),
        last_fill_px: Price(0),
        last_fill_qty: Qty(0),
        sequence,
        recv_ts_ns: sequence,
        source: OrderEventSource::AccountFeed,
    })
}

fn parse_price(value: Option<&str>, scale: i64) -> Result<Price, AccountParseError> {
    Price::parse_scaled(
        value.ok_or(AccountParseError::MissingField)?.as_bytes(),
        scale,
    )
    .map_err(|_| AccountParseError::InvalidDecimal)
}

fn parse_qty(value: Option<&str>, scale: i64) -> Result<Qty, AccountParseError> {
    Qty::parse_scaled(
        value.ok_or(AccountParseError::MissingField)?.as_bytes(),
        scale,
    )
    .map_err(|_| AccountParseError::InvalidDecimal)
}

fn parse_side(value: Option<&str>) -> Result<Side, AccountParseError> {
    match value.ok_or(AccountParseError::MissingField)? {
        "buy" => Ok(Side::Buy),
        "sell" => Ok(Side::Sell),
        _ => Err(AccountParseError::MissingField),
    }
}

#[derive(Deserialize)]
struct RestAccount {
    currency: String,
    balance: String,
    hold: String,
    available: String,
}

#[derive(Deserialize)]
struct RawUserFeedMessage {
    #[serde(rename = "type")]
    message_type: String,
    currency: Option<String>,
    balance: Option<String>,
    available: Option<String>,
    hold: Option<String>,
    order_id: Option<String>,
    client_oid: Option<String>,
    price: Option<String>,
    remaining_size: Option<String>,
    size: Option<String>,
    side: Option<String>,
    reason: Option<String>,
    trade_id: Option<u64>,
    maker_order_id: Option<String>,
    taker_order_id: Option<String>,
}

pub trait RestAccountSnapshotClient {
    type Error;

    fn load_snapshot(&self) -> Result<AccountSnapshot, Self::Error>;
}

pub trait UserFeedEventParser {
    type Error;

    fn parse_user_feed_event(&self, bytes: &[u8]) -> Result<UserFeedEvent, Self::Error>;
}
