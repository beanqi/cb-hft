use std::collections::{HashMap, HashSet};

use crate::event::{ExchangeEvent, FillEvent};
use crate::fix::coinbase::order_entry::{OrderEntryError, parse_execution_report};
use crate::fix::{FixEncoder, FixFrame, FixParser};
use crate::types::{Price, ProductSpec, ProductValidationError, Qty, Side, SymbolId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderStatus {
    PendingNew,
    Open,
    PartiallyFilled,
    Filled,
    PendingCancel,
    Canceled,
    Rejected,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderEventSource {
    FixOrderEntry,
    AccountFeed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderEvent {
    pub symbol_id: SymbolId,
    pub client_order_id: String,
    pub exchange_order_id: String,
    pub exec_id: String,
    pub status: OrderStatus,
    pub side: Side,
    pub price: Price,
    pub original_qty: Qty,
    pub remaining_qty: Qty,
    pub filled_qty: Qty,
    pub avg_fill_px: Price,
    pub last_fill_px: Price,
    pub last_fill_qty: Qty,
    pub sequence: u64,
    pub recv_ts_ns: u64,
    pub source: OrderEventSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RiskConfig {
    pub max_open_orders_per_symbol: usize,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_open_orders_per_symbol: 20,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskError {
    UnknownSymbol,
    PriceNotOnTick,
    QtyBelowMinimum,
    QtyNotOnStep,
    NotionalBelowMinimum,
    MaxOpenOrdersExceeded,
    UnknownClientOrderId,
    UnsupportedCommand,
}

impl From<ProductValidationError> for RiskError {
    fn from(value: ProductValidationError) -> Self {
        match value {
            ProductValidationError::PriceNotOnTick => Self::PriceNotOnTick,
            ProductValidationError::QtyBelowMinimum => Self::QtyBelowMinimum,
            ProductValidationError::QtyNotOnStep => Self::QtyNotOnStep,
            ProductValidationError::NotionalBelowMinimum => Self::NotionalBelowMinimum,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedOrder {
    pub client_order_id: String,
    pub command: NewOrderCommand,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedCancel {
    pub cancel_client_order_id: String,
    pub original_client_order_id: String,
    pub symbol_id: SymbolId,
}

pub struct OrderManager {
    seen_exec_ids: HashSet<String>,
    latest_status: HashMap<String, OrderStatus>,
    symbol_by_client_order_id: HashMap<String, SymbolId>,
    open_counts: HashMap<SymbolId, usize>,
    next_client_order_seq: u64,
    risk: RiskConfig,
}

impl Default for OrderManager {
    fn default() -> Self {
        Self::with_risk(RiskConfig::default())
    }
}

impl OrderManager {
    pub fn with_risk(risk: RiskConfig) -> Self {
        Self {
            seen_exec_ids: HashSet::new(),
            latest_status: HashMap::new(),
            symbol_by_client_order_id: HashMap::new(),
            open_counts: HashMap::new(),
            next_client_order_seq: 1,
            risk,
        }
    }

    pub fn submit_new_order(
        &mut self,
        command: StrategyCommand,
        spec: &ProductSpec,
    ) -> Result<AcceptedOrder, RiskError> {
        let StrategyCommand::NewOrder(command) = command else {
            return Err(RiskError::UnsupportedCommand);
        };
        spec.validate_order(command.price, command.qty)?;
        let open_count = self.open_order_count(command.symbol_id);
        if open_count >= self.risk.max_open_orders_per_symbol {
            return Err(RiskError::MaxOpenOrdersExceeded);
        }

        let client_order_id = self.next_client_order_id();
        self.latest_status
            .insert(client_order_id.clone(), OrderStatus::PendingNew);
        self.symbol_by_client_order_id
            .insert(client_order_id.clone(), command.symbol_id);
        *self.open_counts.entry(command.symbol_id).or_default() += 1;
        Ok(AcceptedOrder {
            client_order_id,
            command,
        })
    }

    pub fn request_cancel(&mut self, client_order_id: &str) -> Result<AcceptedCancel, RiskError> {
        let Some(symbol_id) = self.symbol_by_client_order_id.get(client_order_id).copied() else {
            return Err(RiskError::UnknownClientOrderId);
        };
        let cancel_client_order_id = self.next_client_order_id();
        self.latest_status
            .insert(client_order_id.to_string(), OrderStatus::PendingCancel);
        Ok(AcceptedCancel {
            cancel_client_order_id,
            original_client_order_id: client_order_id.to_string(),
            symbol_id,
        })
    }

    pub fn apply_order_event(&mut self, event: OrderEvent) -> bool {
        if !self.seen_exec_ids.insert(event.exec_id.clone()) {
            return false;
        }
        let previous = self
            .latest_status
            .insert(event.client_order_id.clone(), event.status);
        self.symbol_by_client_order_id
            .insert(event.client_order_id.clone(), event.symbol_id);
        if matches!(
            event.status,
            OrderStatus::Filled | OrderStatus::Canceled | OrderStatus::Rejected
        ) && previous.is_some_and(is_open_like)
        {
            let count = self.open_counts.entry(event.symbol_id).or_default();
            *count = count.saturating_sub(1);
        }
        true
    }

    pub fn status(&self, client_order_id: &str) -> Option<OrderStatus> {
        self.latest_status.get(client_order_id).copied()
    }

    pub fn open_order_count(&self, symbol_id: SymbolId) -> usize {
        self.open_counts.get(&symbol_id).copied().unwrap_or(0)
    }

    pub fn client_order_id_from_sequence(seq: u64) -> String {
        format!("cbhft-{seq}")
    }

    pub fn active_client_order_ids(&self, symbol_id: SymbolId) -> Vec<String> {
        let mut ids: Vec<_> = self
            .symbol_by_client_order_id
            .iter()
            .filter_map(|(client_order_id, order_symbol_id)| {
                let status = self.latest_status.get(client_order_id).copied()?;
                (*order_symbol_id == symbol_id && is_open_like(status))
                    .then(|| client_order_id.clone())
            })
            .collect();
        ids.sort_by_key(|id| client_order_sequence(id).unwrap_or(u64::MAX));
        ids
    }

    fn next_client_order_id(&mut self) -> String {
        let id = format!("cbhft-{}", self.next_client_order_seq);
        self.next_client_order_seq += 1;
        id
    }
}

fn is_open_like(status: OrderStatus) -> bool {
    matches!(
        status,
        OrderStatus::PendingNew
            | OrderStatus::Open
            | OrderStatus::PartiallyFilled
            | OrderStatus::PendingCancel
    )
}

fn client_order_sequence(client_order_id: &str) -> Option<u64> {
    client_order_id.strip_prefix("cbhft-")?.parse().ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeInForce {
    GoodTillCancel,
    ImmediateOrCancel,
    FillOrKill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewOrderCommand {
    pub symbol_id: SymbolId,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
    pub post_only: bool,
    pub time_in_force: TimeInForce,
    pub strategy_order_id: u64,
    pub signal_ts_ns: u64,
}

impl NewOrderCommand {
    pub fn strategy_order_id(&self) -> u64 {
        self.strategy_order_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CancelOrderCommand {
    pub symbol_id: SymbolId,
    pub client_order_id: u64,
    pub strategy_order_id: u64,
    pub signal_ts_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplaceOrderCommand {
    pub symbol_id: SymbolId,
    pub client_order_id: u64,
    pub new_price: Price,
    pub new_qty: Qty,
    pub strategy_order_id: u64,
    pub signal_ts_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyCommand {
    NewOrder(NewOrderCommand),
    CancelOrder(CancelOrderCommand),
    ReplaceOrder(ReplaceOrderCommand),
    CancelAll {
        symbol_id: SymbolId,
        signal_ts_ns: u64,
    },
}

impl StrategyCommand {
    pub fn symbol_id(&self) -> SymbolId {
        match self {
            Self::NewOrder(cmd) => cmd.symbol_id,
            Self::CancelOrder(cmd) => cmd.symbol_id,
            Self::ReplaceOrder(cmd) => cmd.symbol_id,
            Self::CancelAll { symbol_id, .. } => *symbol_id,
        }
    }

    pub fn strategy_order_id(&self) -> u64 {
        match self {
            Self::NewOrder(cmd) => cmd.strategy_order_id,
            Self::CancelOrder(cmd) => cmd.strategy_order_id,
            Self::ReplaceOrder(cmd) => cmd.strategy_order_id,
            Self::CancelAll { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderThreadAction {
    SendFix(Vec<u8>),
    Reject(RiskError),
}

pub struct OrderThreadEngine {
    encoder: FixEncoder,
    manager: OrderManager,
    products: Vec<ProductSpec>,
    next_seq_num: u64,
}

impl OrderThreadEngine {
    pub fn new(encoder: FixEncoder, manager: OrderManager, products: Vec<ProductSpec>) -> Self {
        Self {
            encoder,
            manager,
            products,
            next_seq_num: 1,
        }
    }

    pub fn on_command(
        &mut self,
        command: StrategyCommand,
        sending_time: &str,
    ) -> Vec<OrderThreadAction> {
        match command {
            StrategyCommand::NewOrder(new_order) => self.on_new_order(new_order, sending_time),
            StrategyCommand::CancelOrder(cancel_order) => {
                self.on_cancel_order(cancel_order, sending_time)
            }
            StrategyCommand::CancelAll { symbol_id, .. } => {
                self.on_cancel_all(symbol_id, sending_time)
            }
            StrategyCommand::ReplaceOrder(_) => {
                vec![OrderThreadAction::Reject(RiskError::UnsupportedCommand)]
            }
        }
    }

    fn on_new_order(
        &mut self,
        command: NewOrderCommand,
        sending_time: &str,
    ) -> Vec<OrderThreadAction> {
        let Some(spec) = self.product(command.symbol_id).copied() else {
            return vec![OrderThreadAction::Reject(RiskError::UnknownSymbol)];
        };
        let accepted = match self
            .manager
            .submit_new_order(StrategyCommand::NewOrder(command), &spec)
        {
            Ok(accepted) => accepted,
            Err(err) => return vec![OrderThreadAction::Reject(err)],
        };
        let seq = self.take_seq_num();
        let price = format_scaled(accepted.command.price.0, spec.price_scale);
        let qty = format_scaled(accepted.command.qty.0, spec.qty_scale);
        let fix = self
            .encoder
            .encode_limit_new_order_single_with_time_in_force(
                seq,
                sending_time,
                &accepted.client_order_id,
                spec.coinbase_product,
                accepted.command.side,
                &price,
                &qty,
                accepted.command.post_only,
                accepted.command.time_in_force,
            );
        vec![OrderThreadAction::SendFix(fix)]
    }

    fn on_cancel_order(
        &mut self,
        command: CancelOrderCommand,
        sending_time: &str,
    ) -> Vec<OrderThreadAction> {
        let Some(spec) = self.product(command.symbol_id).copied() else {
            return vec![OrderThreadAction::Reject(RiskError::UnknownSymbol)];
        };
        let original_client_order_id =
            OrderManager::client_order_id_from_sequence(command.client_order_id);
        self.encode_cancel(&spec, &original_client_order_id, sending_time)
    }

    fn on_cancel_all(&mut self, symbol_id: SymbolId, sending_time: &str) -> Vec<OrderThreadAction> {
        let Some(spec) = self.product(symbol_id).copied() else {
            return vec![OrderThreadAction::Reject(RiskError::UnknownSymbol)];
        };
        let ids = self.manager.active_client_order_ids(symbol_id);
        ids.into_iter()
            .flat_map(|client_order_id| self.encode_cancel(&spec, &client_order_id, sending_time))
            .collect()
    }

    fn encode_cancel(
        &mut self,
        spec: &ProductSpec,
        original_client_order_id: &str,
        sending_time: &str,
    ) -> Vec<OrderThreadAction> {
        let accepted = match self.manager.request_cancel(original_client_order_id) {
            Ok(accepted) => accepted,
            Err(err) => return vec![OrderThreadAction::Reject(err)],
        };
        let seq = self.take_seq_num();
        let fix = self.encoder.encode_order_cancel_request(
            seq,
            sending_time,
            &accepted.cancel_client_order_id,
            &accepted.original_client_order_id,
            spec.coinbase_product,
            Side::Buy,
        );
        vec![OrderThreadAction::SendFix(fix)]
    }

    pub fn on_execution_report(
        &mut self,
        parser: &FixParser,
        frame: &FixFrame<'_>,
        recv_ts_ns: u64,
    ) -> Result<Vec<ExchangeEvent>, OrderEntryError> {
        let symbol = fix_symbol(parser, frame).ok_or(OrderEntryError::MissingSymbol)?;
        let Some(spec) = self.product_by_coinbase_symbol(symbol).copied() else {
            return Err(OrderEntryError::UnknownSymbol);
        };
        let order_event = parse_execution_report(parser, frame, &spec, recv_ts_ns)?;
        let is_new = self.manager.apply_order_event(order_event.clone());
        if !is_new {
            return Ok(Vec::new());
        }

        let mut events = Vec::with_capacity(if order_event.last_fill_qty.0 > 0 {
            2
        } else {
            1
        });
        events.push(ExchangeEvent::Order(order_event.clone()));
        if order_event.last_fill_qty.0 > 0 {
            events.push(ExchangeEvent::Fill(FillEvent {
                symbol_id: order_event.symbol_id,
                client_order_id: order_event.client_order_id,
                exchange_order_id: order_event.exchange_order_id,
                exec_id: order_event.exec_id,
                side: order_event.side,
                price: order_event.last_fill_px,
                qty: order_event.last_fill_qty,
                recv_ts_ns: order_event.recv_ts_ns,
            }));
        }
        Ok(events)
    }

    pub fn manager(&self) -> &OrderManager {
        &self.manager
    }

    pub fn next_seq_num(&self) -> u64 {
        self.next_seq_num
    }

    pub fn set_next_seq_num(&mut self, next_seq_num: u64) {
        self.next_seq_num = next_seq_num;
    }

    fn product(&self, symbol_id: SymbolId) -> Option<&ProductSpec> {
        self.products
            .iter()
            .find(|spec| spec.symbol_id == symbol_id)
    }

    fn product_by_coinbase_symbol(&self, symbol: &str) -> Option<&ProductSpec> {
        self.products
            .iter()
            .find(|spec| spec.coinbase_product == symbol)
    }

    fn take_seq_num(&mut self) -> u64 {
        let seq = self.next_seq_num;
        self.next_seq_num += 1;
        seq
    }
}

fn format_scaled(value: i64, scale: i64) -> String {
    if scale <= 1 {
        return value.to_string();
    }
    let decimals = scale.ilog10() as usize;
    let sign = if value < 0 { "-" } else { "" };
    let abs = value.abs();
    let int = abs / scale;
    let frac = abs % scale;
    format!("{sign}{int}.{frac:0decimals$}")
}

fn fix_symbol<'a>(parser: &FixParser, frame: &'a FixFrame<'a>) -> Option<&'a str> {
    parser
        .fields(frame)
        .find(|field| field.tag == 55)
        .and_then(|field| std::str::from_utf8(field.value).ok())
}
