use crate::event::{BalanceEvent, FillEvent};
use crate::market::{L1Book, Trade};
use crate::order::{OrderEvent, OrderStatus, StrategyCommand};
use crate::types::SymbolId;

pub trait Strategy {
    fn on_l1(&mut self, _ctx: &mut StrategyContext<'_>, _book: &L1Book) {}
    fn on_trade(&mut self, _ctx: &mut StrategyContext<'_>, _trade: &Trade) {}
    fn on_order_event(&mut self, _ctx: &mut StrategyContext<'_>, _event: &OrderEvent) {}
    fn on_fill_event(&mut self, _ctx: &mut StrategyContext<'_>, _event: &FillEvent) {}
    fn on_balance_event(&mut self, _ctx: &mut StrategyContext<'_>, _event: &BalanceEvent) {}
}

impl<T: Strategy + ?Sized> Strategy for Box<T> {
    fn on_l1(&mut self, ctx: &mut StrategyContext<'_>, book: &L1Book) {
        (**self).on_l1(ctx, book);
    }

    fn on_trade(&mut self, ctx: &mut StrategyContext<'_>, trade: &Trade) {
        (**self).on_trade(ctx, trade);
    }

    fn on_order_event(&mut self, ctx: &mut StrategyContext<'_>, event: &OrderEvent) {
        (**self).on_order_event(ctx, event);
    }

    fn on_fill_event(&mut self, ctx: &mut StrategyContext<'_>, event: &FillEvent) {
        (**self).on_fill_event(ctx, event);
    }

    fn on_balance_event(&mut self, ctx: &mut StrategyContext<'_>, event: &BalanceEvent) {
        (**self).on_balance_event(ctx, event);
    }
}

pub struct StrategyContext<'a> {
    symbol_id: SymbolId,
    now_ns: u64,
    emitted: &'a mut Vec<StrategyCommand>,
}

impl<'a> StrategyContext<'a> {
    pub fn new(symbol_id: SymbolId, now_ns: u64, emitted: &'a mut Vec<StrategyCommand>) -> Self {
        Self {
            symbol_id,
            now_ns,
            emitted,
        }
    }

    pub fn emit(&mut self, command: StrategyCommand) {
        self.emitted.push(command);
    }

    pub fn symbol_id(&self) -> SymbolId {
        self.symbol_id
    }

    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }
}

#[derive(Default)]
pub struct NoopStrategy;

impl Strategy for NoopStrategy {}

#[derive(Default)]
pub struct RecordingStrategy {
    pub l1_count: usize,
    pub trade_count: usize,
    pub order_statuses: Vec<OrderStatus>,
    pub fill_count: usize,
    pub balance_count: usize,
}

impl Strategy for RecordingStrategy {
    fn on_l1(&mut self, _ctx: &mut StrategyContext<'_>, _book: &L1Book) {
        self.l1_count += 1;
    }

    fn on_trade(&mut self, _ctx: &mut StrategyContext<'_>, _trade: &Trade) {
        self.trade_count += 1;
    }

    fn on_order_event(&mut self, _ctx: &mut StrategyContext<'_>, event: &OrderEvent) {
        self.order_statuses.push(event.status);
    }

    fn on_fill_event(&mut self, _ctx: &mut StrategyContext<'_>, _event: &FillEvent) {
        self.fill_count += 1;
    }

    fn on_balance_event(&mut self, _ctx: &mut StrategyContext<'_>, _event: &BalanceEvent) {
        self.balance_count += 1;
    }
}

pub struct QuoteOnFirstL1Strategy {
    command: StrategyCommand,
    emitted_once: bool,
}

impl QuoteOnFirstL1Strategy {
    pub fn new(command: StrategyCommand) -> Self {
        Self {
            command,
            emitted_once: false,
        }
    }
}

impl Strategy for QuoteOnFirstL1Strategy {
    fn on_l1(&mut self, ctx: &mut StrategyContext<'_>, _book: &L1Book) {
        if !self.emitted_once {
            ctx.emit(self.command);
            self.emitted_once = true;
        }
    }
}
