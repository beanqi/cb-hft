use crate::event::FillEvent;
use crate::market::{L1Book, Trade};
use crate::order::{
    CancelOrderCommand, NewOrderCommand, OrderEvent, OrderStatus, StrategyCommand, TimeInForce,
};
use crate::strategy::{Strategy, StrategyContext};
use crate::types::{Price, Qty, Side, SymbolId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trend {
    Neutral,
    StrongUp,
    StrongDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrendConfig {
    pub window_ns: u64,
    pub min_window_notional: i128,
    pub strong_score_x100: i64,
    pub trade_weight_x100: i64,
    pub count_weight_x100: i64,
    pub obi_weight_x100: i64,
    pub micro_weight_x100: i64,
}

impl Default for TrendConfig {
    fn default() -> Self {
        Self {
            window_ns: 2_000_000_000,
            min_window_notional: 5_000_000,
            strong_score_x100: 150,
            trade_weight_x100: 100,
            count_weight_x100: 50,
            obi_weight_x100: 80,
            micro_weight_x100: 40,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MakerConfig {
    pub symbol_id: SymbolId,
    pub quote_qty: Qty,
    pub quote_tick_offset: i64,
    pub requote_ticks: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Flat,
    TwoSided,
    Exiting { side: Side },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveOrder {
    seq: u64,
    side: Side,
    price: Price,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TradeSample {
    ts_ns: u64,
    side: Side,
    notional: i128,
}

pub struct MakerStrategy {
    config: MakerConfig,
    trend_config: TrendConfig,
    trades: Vec<TradeSample>,
    last_book: Option<L1Book>,
    phase: Phase,
    bid_order: Option<ActiveOrder>,
    ask_order: Option<ActiveOrder>,
    exit_order: Option<ActiveOrder>,
    next_strategy_order_id: u64,
}

impl MakerStrategy {
    pub fn new(config: MakerConfig, trend_config: TrendConfig) -> Self {
        Self {
            config,
            trend_config,
            trades: Vec::with_capacity(256),
            last_book: None,
            phase: Phase::Flat,
            bid_order: None,
            ask_order: None,
            exit_order: None,
            next_strategy_order_id: 1,
        }
    }

    pub fn trend(&self) -> Trend {
        self.score_to_trend(self.trend_score_x100())
    }

    pub fn trend_score_x100(&self) -> i64 {
        let (buy_notional, sell_notional, buy_count, sell_count) = self.trade_stats();
        let total_notional = buy_notional + sell_notional;
        if total_notional < self.trend_config.min_window_notional {
            return 0;
        }

        let trade_imb_x100 = ratio_x100(buy_notional - sell_notional, total_notional);
        let count_total = buy_count + sell_count;
        let count_imb_x100 = if count_total > 0 {
            ((buy_count - sell_count) * 100 / count_total) as i64
        } else {
            0
        };

        let (obi_x100, micro_bps) = self
            .last_book
            .map(|book| book_features(&book))
            .unwrap_or((0, 0));

        (self.trend_config.trade_weight_x100 * trade_imb_x100
            + self.trend_config.count_weight_x100 * count_imb_x100
            + self.trend_config.obi_weight_x100 * obi_x100
            + self.trend_config.micro_weight_x100 * micro_bps)
            / 100
    }

    pub fn on_trade_sample(&mut self, trade: &Trade) {
        if trade.symbol_id != self.config.symbol_id {
            return;
        }
        let Some(side) = trade.side else {
            return;
        };
        self.trades.push(TradeSample {
            ts_ns: trade.recv_ts_ns,
            side,
            notional: (trade.price.0 as i128) * (trade.qty.0 as i128),
        });
        self.prune_trades(trade.recv_ts_ns);
    }

    pub fn on_book_sample(
        &mut self,
        bid_px: Price,
        bid_qty: Qty,
        ask_px: Price,
        ask_qty: Qty,
        now_ns: u64,
    ) {
        self.last_book = Some(L1Book {
            bid_px,
            bid_qty,
            ask_px,
            ask_qty,
            last_sequence: 0,
            last_update_recv_ns: now_ns,
        });
        self.prune_trades(now_ns);
    }

    fn on_book(&mut self, ctx: &mut StrategyContext<'_>, book: &L1Book) {
        self.last_book = Some(*book);
        self.prune_trades(ctx.now_ns());

        if self.trend() != Trend::Neutral {
            self.cancel_all(ctx);
            self.phase = Phase::Flat;
            self.bid_order = None;
            self.ask_order = None;
            self.exit_order = None;
            return;
        }

        match self.phase {
            Phase::Flat => self.quote_two_sides(ctx, book),
            Phase::TwoSided => self.requote_two_sides(ctx, book),
            Phase::Exiting { side } => self.requote_exit(ctx, book, side),
        }
    }

    fn quote_two_sides(&mut self, ctx: &mut StrategyContext<'_>, book: &L1Book) {
        let bid_px = Price(book.bid_px.0 - self.config.quote_tick_offset);
        let ask_px = Price(book.ask_px.0 + self.config.quote_tick_offset);
        ctx.emit(self.new_order(ctx, Side::Buy, bid_px));
        ctx.emit(self.new_order(ctx, Side::Sell, ask_px));
        self.bid_order = Some(ActiveOrder {
            seq: self.next_order_seq_guess(),
            side: Side::Buy,
            price: bid_px,
        });
        self.ask_order = Some(ActiveOrder {
            seq: self.next_order_seq_guess() + 1,
            side: Side::Sell,
            price: ask_px,
        });
        self.phase = Phase::TwoSided;
    }

    fn requote_two_sides(&mut self, ctx: &mut StrategyContext<'_>, book: &L1Book) {
        let target_bid = Price(book.bid_px.0 - self.config.quote_tick_offset);
        let target_ask = Price(book.ask_px.0 + self.config.quote_tick_offset);
        if should_requote(self.bid_order, target_bid, self.config.requote_ticks) {
            if let Some(order) = self.bid_order.take() {
                ctx.emit(cancel(ctx, order.seq));
            }
            ctx.emit(self.new_order(ctx, Side::Buy, target_bid));
            self.bid_order = Some(ActiveOrder {
                seq: self.next_order_seq_guess(),
                side: Side::Buy,
                price: target_bid,
            });
        }
        if should_requote(self.ask_order, target_ask, self.config.requote_ticks) {
            if let Some(order) = self.ask_order.take() {
                ctx.emit(cancel(ctx, order.seq));
            }
            ctx.emit(self.new_order(ctx, Side::Sell, target_ask));
            self.ask_order = Some(ActiveOrder {
                seq: self.next_order_seq_guess(),
                side: Side::Sell,
                price: target_ask,
            });
        }
    }

    fn requote_exit(&mut self, ctx: &mut StrategyContext<'_>, book: &L1Book, side: Side) {
        let target = match side {
            Side::Buy => book.bid_px,
            Side::Sell => book.ask_px,
        };
        if should_requote(self.exit_order, target, self.config.requote_ticks) {
            if let Some(order) = self.exit_order.take() {
                ctx.emit(cancel(ctx, order.seq));
            }
            ctx.emit(self.new_order(ctx, side, target));
            self.exit_order = Some(ActiveOrder {
                seq: self.next_order_seq_guess(),
                side,
                price: target,
            });
        }
    }

    fn on_fill(&mut self, ctx: &mut StrategyContext<'_>, fill: &FillEvent) {
        if fill.symbol_id != self.config.symbol_id {
            return;
        }
        match self.phase {
            Phase::TwoSided => match fill.side {
                Side::Buy => {
                    if let Some(order) = self.ask_order.take() {
                        ctx.emit(cancel(ctx, order.seq));
                    }
                    self.bid_order = None;
                    self.start_exit(ctx, Side::Sell);
                }
                Side::Sell => {
                    if let Some(order) = self.bid_order.take() {
                        ctx.emit(cancel(ctx, order.seq));
                    }
                    self.ask_order = None;
                    self.start_exit(ctx, Side::Buy);
                }
            },
            Phase::Exiting { side } if side == fill.side => {
                self.exit_order = None;
                self.phase = Phase::Flat;
            }
            _ => {}
        }
    }

    fn start_exit(&mut self, ctx: &mut StrategyContext<'_>, side: Side) {
        let Some(book) = self.last_book else {
            self.phase = Phase::Exiting { side };
            return;
        };
        let px = match side {
            Side::Buy => book.bid_px,
            Side::Sell => book.ask_px,
        };
        ctx.emit(self.new_order(ctx, side, px));
        self.exit_order = Some(ActiveOrder {
            seq: self.next_order_seq_guess(),
            side,
            price: px,
        });
        self.phase = Phase::Exiting { side };
    }

    fn on_order(&mut self, event: &OrderEvent) {
        let Some(seq) = parse_cbhft_seq(&event.client_order_id) else {
            return;
        };
        if event.status == OrderStatus::Open {
            match self.phase {
                Phase::TwoSided => match event.side {
                    Side::Buy => {
                        self.bid_order = Some(ActiveOrder {
                            seq,
                            side: event.side,
                            price: event.price,
                        })
                    }
                    Side::Sell => {
                        self.ask_order = Some(ActiveOrder {
                            seq,
                            side: event.side,
                            price: event.price,
                        })
                    }
                },
                Phase::Exiting { side } if side == event.side => {
                    self.exit_order = Some(ActiveOrder {
                        seq,
                        side: event.side,
                        price: event.price,
                    });
                }
                _ => {}
            }
        }
    }

    fn cancel_all(&mut self, ctx: &mut StrategyContext<'_>) {
        ctx.emit(StrategyCommand::CancelAll {
            symbol_id: self.config.symbol_id,
            signal_ts_ns: ctx.now_ns(),
        });
    }

    fn new_order(
        &mut self,
        ctx: &StrategyContext<'_>,
        side: Side,
        price: Price,
    ) -> StrategyCommand {
        let id = self.next_strategy_order_id;
        self.next_strategy_order_id += 1;
        StrategyCommand::NewOrder(NewOrderCommand {
            symbol_id: self.config.symbol_id,
            side,
            price,
            qty: self.config.quote_qty,
            post_only: true,
            time_in_force: TimeInForce::GoodTillCancel,
            strategy_order_id: id,
            signal_ts_ns: ctx.now_ns(),
        })
    }

    fn next_order_seq_guess(&self) -> u64 {
        self.next_strategy_order_id.saturating_sub(1)
    }

    fn trade_stats(&self) -> (i128, i128, i64, i64) {
        let mut buy_notional = 0;
        let mut sell_notional = 0;
        let mut buy_count = 0;
        let mut sell_count = 0;
        for sample in &self.trades {
            match sample.side {
                Side::Buy => {
                    buy_notional += sample.notional;
                    buy_count += 1;
                }
                Side::Sell => {
                    sell_notional += sample.notional;
                    sell_count += 1;
                }
            }
        }
        (buy_notional, sell_notional, buy_count, sell_count)
    }

    fn prune_trades(&mut self, now_ns: u64) {
        let cutoff = now_ns.saturating_sub(self.trend_config.window_ns);
        let first_live = self
            .trades
            .iter()
            .position(|sample| sample.ts_ns >= cutoff)
            .unwrap_or(self.trades.len());
        if first_live > 0 {
            self.trades.drain(0..first_live);
        }
    }

    fn score_to_trend(&self, score: i64) -> Trend {
        if score >= self.trend_config.strong_score_x100 {
            Trend::StrongUp
        } else if score <= -self.trend_config.strong_score_x100 {
            Trend::StrongDown
        } else {
            Trend::Neutral
        }
    }
}

impl Strategy for MakerStrategy {
    fn on_l1(&mut self, ctx: &mut StrategyContext<'_>, book: &L1Book) {
        self.on_book(ctx, book);
    }

    fn on_trade(&mut self, _ctx: &mut StrategyContext<'_>, trade: &Trade) {
        self.on_trade_sample(trade);
    }

    fn on_order_event(&mut self, _ctx: &mut StrategyContext<'_>, event: &OrderEvent) {
        self.on_order(event);
    }

    fn on_fill_event(&mut self, ctx: &mut StrategyContext<'_>, event: &FillEvent) {
        self.on_fill(ctx, event);
    }
}

fn should_requote(order: Option<ActiveOrder>, target: Price, min_delta: i64) -> bool {
    match order {
        None => true,
        Some(order) => (order.price.0 - target.0).abs() >= min_delta,
    }
}

fn cancel(ctx: &StrategyContext<'_>, seq: u64) -> StrategyCommand {
    StrategyCommand::CancelOrder(CancelOrderCommand {
        symbol_id: ctx.symbol_id(),
        client_order_id: seq,
        strategy_order_id: 0,
        signal_ts_ns: ctx.now_ns(),
    })
}

fn ratio_x100(num: i128, den: i128) -> i64 {
    if den == 0 {
        0
    } else {
        (num * 100 / den) as i64
    }
}

fn book_features(book: &L1Book) -> (i64, i64) {
    let qty_total = book.bid_qty.0 as i128 + book.ask_qty.0 as i128;
    let obi_x100 = ratio_x100(book.bid_qty.0 as i128 - book.ask_qty.0 as i128, qty_total);
    let mid = (book.bid_px.0 as i128 + book.ask_px.0 as i128) / 2;
    if mid <= 0 || qty_total <= 0 {
        return (obi_x100, 0);
    }
    let micro_num = book.ask_px.0 as i128 * book.bid_qty.0 as i128
        + book.bid_px.0 as i128 * book.ask_qty.0 as i128;
    let micro = micro_num / qty_total;
    let micro_bps = ((micro - mid) * 10_000 / mid) as i64;
    (obi_x100, micro_bps)
}

fn parse_cbhft_seq(input: &str) -> Option<u64> {
    input.strip_prefix("cbhft-")?.parse().ok()
}
