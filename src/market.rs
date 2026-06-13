use crate::event::ExchangeEvent;
use crate::order::StrategyCommand;
use crate::strategy::{Strategy, StrategyContext};
use crate::types::{Price, Qty, SymbolId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketEvent {
    L1 {
        symbol_id: SymbolId,
        recv_ts_ns: u64,
        bid_px: Price,
        bid_qty: Qty,
        ask_px: Price,
        ask_qty: Qty,
        sequence: u64,
    },
    Trade(Trade),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trade {
    pub symbol_id: SymbolId,
    pub recv_ts_ns: u64,
    pub trade_id: u64,
    pub price: Price,
    pub qty: Qty,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct L1Update {
    pub symbol_id: SymbolId,
    pub exchange_ts_ns: u64,
    pub recv_ts_ns: u64,
    pub bid_px: Price,
    pub bid_qty: Qty,
    pub ask_px: Price,
    pub ask_qty: Qty,
    pub sequence: u64,
}

impl TryFrom<MarketEvent> for L1Update {
    type Error = ();

    fn try_from(value: MarketEvent) -> Result<Self, Self::Error> {
        match value {
            MarketEvent::L1 {
                symbol_id,
                recv_ts_ns,
                bid_px,
                bid_qty,
                ask_px,
                ask_qty,
                sequence,
            } => Ok(Self {
                symbol_id,
                exchange_ts_ns: 0,
                recv_ts_ns,
                bid_px,
                bid_qty,
                ask_px,
                ask_qty,
                sequence,
            }),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct L1Book {
    pub bid_px: Price,
    pub bid_qty: Qty,
    pub ask_px: Price,
    pub ask_qty: Qty,
    pub last_sequence: u64,
    pub last_update_recv_ns: u64,
}

impl L1Book {
    #[inline(always)]
    pub fn apply(&mut self, update: L1Update) {
        if update.sequence > self.last_sequence {
            self.bid_px = update.bid_px;
            self.bid_qty = update.bid_qty;
            self.ask_px = update.ask_px;
            self.ask_qty = update.ask_qty;
            self.last_sequence = update.sequence;
            self.last_update_recv_ns = update.recv_ts_ns;
        }
    }
}

pub struct MarketEngine<S> {
    symbol_id: SymbolId,
    strategy: S,
    book: L1Book,
}

impl<S: Strategy> MarketEngine<S> {
    pub fn new(symbol_id: SymbolId, strategy: S) -> Self {
        Self {
            symbol_id,
            strategy,
            book: L1Book::default(),
        }
    }

    pub fn on_market_event(&mut self, event: MarketEvent) -> Vec<StrategyCommand> {
        let mut emitted = Vec::new();
        match event {
            MarketEvent::L1 { symbol_id, .. } if symbol_id == self.symbol_id => {
                let update = L1Update::try_from(event).expect("L1 event converts to L1Update");
                self.book.apply(update);
                let mut ctx = StrategyContext::new(symbol_id, update.recv_ts_ns, &mut emitted);
                self.strategy.on_l1(&mut ctx, &self.book);
            }
            MarketEvent::Trade(trade) if trade.symbol_id == self.symbol_id => {
                let mut ctx = StrategyContext::new(trade.symbol_id, trade.recv_ts_ns, &mut emitted);
                self.strategy.on_trade(&mut ctx, &trade);
            }
            _ => {}
        }
        emitted
    }

    pub fn on_exchange_event(&mut self, event: ExchangeEvent) -> Vec<StrategyCommand> {
        let mut emitted = Vec::new();
        match event {
            ExchangeEvent::Order(order) if order.symbol_id == self.symbol_id => {
                let mut ctx = StrategyContext::new(order.symbol_id, order.recv_ts_ns, &mut emitted);
                self.strategy.on_order_event(&mut ctx, &order);
            }
            ExchangeEvent::Fill(fill) if fill.symbol_id == self.symbol_id => {
                let mut ctx = StrategyContext::new(fill.symbol_id, fill.recv_ts_ns, &mut emitted);
                self.strategy.on_fill_event(&mut ctx, &fill);
            }
            ExchangeEvent::Balance(balance) => {
                let mut ctx =
                    StrategyContext::new(self.symbol_id, balance.recv_ts_ns, &mut emitted);
                self.strategy.on_balance_event(&mut ctx, &balance);
            }
            _ => {}
        }
        emitted
    }

    pub fn book(&self) -> &L1Book {
        &self.book
    }

    pub fn strategy(&self) -> &S {
        &self.strategy
    }
}
