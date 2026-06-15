use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::event::{ExchangeEvent, FillEvent};
use crate::market::{MarketEvent, Trade};
use crate::order::OrderEvent;

#[derive(Clone, Debug)]
pub struct DashboardFeed {
    inner: Arc<Mutex<Inner>>,
    cap: usize,
}

#[derive(Debug)]
struct Inner {
    next_seq: u64,
    l1: HashMap<u16, L1View>,
    trades: VecDeque<TradeView>,
    orders: VecDeque<OrderView>,
    fills: VecDeque<FillView>,
    events: VecDeque<SequencedFeedEvent>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FeedSnapshot {
    pub l1: Vec<L1View>,
    pub trades: Vec<TradeView>,
    pub orders: Vec<OrderView>,
    pub fills: Vec<FillView>,
    pub next_seq: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SequencedFeedEvent {
    pub seq: u64,
    pub event: FeedEvent,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "type", content = "data")]
pub enum FeedEvent {
    L1(L1View),
    Trade(TradeView),
    Order(OrderView),
    Fill(FillView),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct L1View {
    pub symbol_id: u16,
    pub bid_px: i64,
    pub bid_qty: i64,
    pub ask_px: i64,
    pub ask_qty: i64,
    pub sequence: u64,
    pub recv_ts_ns: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct TradeView {
    pub symbol_id: u16,
    pub trade_id: u64,
    pub side: Option<String>,
    pub price: i64,
    pub qty: i64,
    pub sequence: u64,
    pub recv_ts_ns: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct OrderView {
    pub symbol_id: u16,
    pub client_order_id: String,
    pub exchange_order_id: String,
    pub exec_id: String,
    pub status: String,
    pub side: String,
    pub price: i64,
    pub original_qty: i64,
    pub remaining_qty: i64,
    pub filled_qty: i64,
    pub avg_fill_px: i64,
    pub last_fill_px: i64,
    pub last_fill_qty: i64,
    pub sequence: u64,
    pub recv_ts_ns: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FillView {
    pub symbol_id: u16,
    pub client_order_id: String,
    pub exchange_order_id: String,
    pub exec_id: String,
    pub side: String,
    pub price: i64,
    pub qty: i64,
    pub recv_ts_ns: u64,
}

impl DashboardFeed {
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                next_seq: 1,
                l1: HashMap::new(),
                trades: VecDeque::new(),
                orders: VecDeque::new(),
                fills: VecDeque::new(),
                events: VecDeque::new(),
            })),
            cap: cap.max(1),
        }
    }

    pub fn publish_market(&self, event: MarketEvent) {
        let mut inner = self.inner.lock().expect("dashboard feed poisoned");
        match event {
            MarketEvent::L1 {
                symbol_id,
                recv_ts_ns,
                bid_px,
                bid_qty,
                ask_px,
                ask_qty,
                sequence,
            } => {
                let view = L1View {
                    symbol_id: symbol_id.0,
                    bid_px: bid_px.0,
                    bid_qty: bid_qty.0,
                    ask_px: ask_px.0,
                    ask_qty: ask_qty.0,
                    sequence,
                    recv_ts_ns,
                };
                inner.l1.insert(symbol_id.0, view.clone());
                push_event(&mut inner, self.cap, FeedEvent::L1(view));
            }
            MarketEvent::Trade(trade) => {
                let view = trade_view(trade);
                push_ring(&mut inner.trades, self.cap, view.clone());
                push_event(&mut inner, self.cap, FeedEvent::Trade(view));
            }
        }
    }

    pub fn publish_exchange(&self, event: ExchangeEvent) {
        let mut inner = self.inner.lock().expect("dashboard feed poisoned");
        match event {
            ExchangeEvent::Order(order) => {
                let view = order_view(order);
                push_ring(&mut inner.orders, self.cap, view.clone());
                push_event(&mut inner, self.cap, FeedEvent::Order(view));
            }
            ExchangeEvent::Fill(fill) => {
                let view = fill_view(fill);
                push_ring(&mut inner.fills, self.cap, view.clone());
                push_event(&mut inner, self.cap, FeedEvent::Fill(view));
            }
            _ => {}
        }
    }

    pub fn snapshot(&self) -> FeedSnapshot {
        let inner = self.inner.lock().expect("dashboard feed poisoned");
        FeedSnapshot {
            l1: inner.l1.values().cloned().collect(),
            trades: inner.trades.iter().cloned().collect(),
            orders: inner.orders.iter().cloned().collect(),
            fills: inner.fills.iter().cloned().collect(),
            next_seq: inner.next_seq,
        }
    }

    pub fn events_after(&self, after: u64) -> Vec<SequencedFeedEvent> {
        let inner = self.inner.lock().expect("dashboard feed poisoned");
        inner
            .events
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect()
    }
}

fn push_ring<T>(ring: &mut VecDeque<T>, cap: usize, value: T) {
    if ring.len() >= cap {
        ring.pop_front();
    }
    ring.push_back(value);
}

fn push_event(inner: &mut Inner, cap: usize, event: FeedEvent) {
    let seq = inner.next_seq;
    inner.next_seq += 1;
    push_ring(
        &mut inner.events,
        cap.saturating_mul(4).max(16),
        SequencedFeedEvent { seq, event },
    );
}

fn trade_view(trade: Trade) -> TradeView {
    TradeView {
        symbol_id: trade.symbol_id.0,
        trade_id: trade.trade_id,
        side: trade.side.map(|side| format!("{side:?}")),
        price: trade.price.0,
        qty: trade.qty.0,
        sequence: trade.sequence,
        recv_ts_ns: trade.recv_ts_ns,
    }
}

fn order_view(order: OrderEvent) -> OrderView {
    OrderView {
        symbol_id: order.symbol_id.0,
        client_order_id: order.client_order_id,
        exchange_order_id: order.exchange_order_id,
        exec_id: order.exec_id,
        status: format!("{:?}", order.status),
        side: format!("{:?}", order.side),
        price: order.price.0,
        original_qty: order.original_qty.0,
        remaining_qty: order.remaining_qty.0,
        filled_qty: order.filled_qty.0,
        avg_fill_px: order.avg_fill_px.0,
        last_fill_px: order.last_fill_px.0,
        last_fill_qty: order.last_fill_qty.0,
        sequence: order.sequence,
        recv_ts_ns: order.recv_ts_ns,
    }
}

fn fill_view(fill: FillEvent) -> FillView {
    FillView {
        symbol_id: fill.symbol_id.0,
        client_order_id: fill.client_order_id,
        exchange_order_id: fill.exchange_order_id,
        exec_id: fill.exec_id,
        side: format!("{:?}", fill.side),
        price: fill.price.0,
        qty: fill.qty.0,
        recv_ts_ns: fill.recv_ts_ns,
    }
}
