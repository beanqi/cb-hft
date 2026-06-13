use crate::order::OrderEvent;
use crate::types::{AssetId, Price, Qty, Side, SymbolId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExchangeEvent {
    Order(OrderEvent),
    Fill(FillEvent),
    Balance(BalanceEvent),
    Session(SessionEvent),
    Risk(RiskEvent),
}

impl ExchangeEvent {
    pub fn symbol_id(&self) -> Option<SymbolId> {
        match self {
            Self::Order(event) => Some(event.symbol_id),
            Self::Fill(event) => Some(event.symbol_id),
            Self::Balance(_) | Self::Session(_) | Self::Risk(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillEvent {
    pub symbol_id: SymbolId,
    pub client_order_id: String,
    pub exchange_order_id: String,
    pub exec_id: String,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
    pub recv_ts_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalanceEvent {
    pub asset_id: AssetId,
    pub total: Qty,
    pub available: Qty,
    pub hold: Qty,
    pub update_ts_ns: u64,
    pub recv_ts_ns: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    Connected,
    Disconnected,
    SequenceGap { expected: u64, received: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RiskEvent {
    CommandRingFull,
    EventRingFull,
    TradingHalted,
}
