# Coinbase 单市场高频挂单系统方案（Rust）

> 目标：实现面向 Coinbase 的低延迟交易所交互层：FIX 行情、FIX 下单、订单回报、资产状态推送，以及与后续策略模块对接的无锁事件通道。本文先定义代码结构、线程模型、核心数据结构、性能目标、测试与待确认问题；策略本身预留接口，不在当前阶段实现。

---

## 1. 需求摘要

### 1.1 交易范围

- 交易所：Coinbase。
- 市场类型：单市场现货挂单交易。
- 标的：BTC、ETH、SOL 等主流币种，交易对数量不超过 10 个。
- 主要职责：
  - 接入 Coinbase FIX Market Data：逐笔成交、L1 深度。
  - 接入 Coinbase FIX Order Entry：下单、撤单、订单回报。
  - 接入订单与资产推送：优先 FIX，如 Coinbase FIX 不提供完整资产推送，则使用 Coinbase WS user channel 或 REST 快照 + WS 增量。
  - 将行情事件撮合成策略输入，并由策略产生下单/撤单信号。
  - 使用环形缓冲队列在线程之间传递信号。
  - 线程绑定 CPU core。

### 1.2 性能目标

- 热路径目标：**逐笔成交 + L1 深度反序列化 + 本地 L1 book 更新/撮合，在 steady-state p99 < 2µs**。
- 该 2µs 目标建议明确为：
  - **不包含** 网络传输、内核 socket wakeup、TLS 解密、日志落盘、跨线程排队后的等待时间。
  - **包含**：从已读入用户态 buffer 的一条 FIX message 开始，完成字段解析、生成 typed event、更新 symbol-local market state、触发策略接口前的撮合判断。
- 所有热路径必须：
  - 无 heap allocation。
  - 不使用动态字符串解析。
  - 不使用 `HashMap<String, _>` / `serde` / JSON。
  - 使用整数定点数表达价格和数量。
  - 尽量避免锁；跨线程使用 SPSC/MPSC ring buffer。

---

## 2. 总体架构

```text
                         ┌────────────────────────────┐
                         │          main.rs           │
                         │ config / cpu pin / startup │
                         └─────────────┬──────────────┘
                                       │
      ┌────────────────────────────────┼────────────────────────────────┐
      │                                │                                │
┌─────▼────────────────┐    ┌──────────▼───────────┐       ┌────────────▼─────────────┐
│ market thread: BTC   │    │ market thread: ETH   │  ...  │ market thread: SOL       │
│ FIX MD TCP/TLS       │    │ FIX MD TCP/TLS       │       │ FIX MD TCP/TLS           │
│ parse trades + L1    │    │ parse trades + L1    │       │ parse trades + L1        │
│ local book/matcher   │    │ local book/matcher   │       │ local book/matcher       │
│ strategy adapter     │    │ strategy adapter     │       │ strategy adapter         │
└─────┬─────────▲───────┘    └──────────┬────────▲──┘       └────────────┬──────▲──────┘
      │         │                       │        │                       │      │
      │ Signal  │ Order/Asset Event     │        │                       │      │
      │         │                       │        │                       │      │
      └─────────┴────────────┬──────────┴────────┴──────────────┬────────┴──────┘
                             │                                  │
                  ring buffer 1: StrategyCommand                │
                             │                                  │
                     ┌───────▼────────┐                         │
                     │ order thread   │                         │
                     │ FIX OE TCP/TLS │                         │
                     │ new/cancel     │                         │
                     │ exec reports   │                         │
                     └───────┬────────┘                         │
                             │                                  │
                             │ ring buffer 2: ExchangeEvent      │
                             ▼                                  │
                     ┌────────────────┐                         │
                     │ account thread │─────────────────────────┘
                     │ FIX or WS user │
                     │ orders/assets  │
                     └────────────────┘
```

### 2.1 线程划分

#### A. Market data + matching thread（每个交易对一个）

每个交易对一个线程，例如：

- `md-BTC-USD`
- `md-ETH-USD`
- `md-SOL-USD`

职责：

1. 维护该交易对的 Coinbase FIX Market Data session。
2. 订阅该交易对的逐笔成交与 L1 bid/ask。
3. 解析 FIX 行情消息。
4. 更新本地 L1 book 与 last trade 状态。
5. 调用策略接口生成 `StrategyCommand`。
6. 将 `StrategyCommand` 写入环形队列 1，发给 order thread。
7. 从环形队列 2 接收属于本 symbol 的 `ExchangeEvent`，驱动策略生命周期，例如订单 accepted / rejected / filled / canceled、账户余额变化等。

备注：

- 一个交易对一个 FIX MD TCP 连接的隔离性最好，延迟抖动更容易控制。
- 但 Coinbase 可能对 session 数量、连接数、登录频率有限制。若限制较紧，应支持切换为“一个 MD 连接订阅多个 symbol，再按 symbol 分发到多个 market thread”的模式。

#### B. Order thread（全局一个）

职责：

1. 维护 Coinbase FIX Order Entry session。
2. 从环形队列 1 读取所有 symbol 的 `StrategyCommand`。
3. 做风控前置校验：价格、数量、资金、订单频率、最大挂单数、client order id 唯一性。
4. 编码并发送 FIX NewOrderSingle / OrderCancelRequest / OrderCancelReplaceRequest（如需要）。
5. 解析 FIX ExecutionReport / OrderCancelReject / BusinessMessageReject。
6. 将订单回报转换为 `ExchangeEvent::Order`，写入环形队列 2。

#### C. Account / user feed thread（全局一个）

职责：

1. 优先验证 Coinbase FIX 是否能提供完整资产推送。
2. 如果 FIX 不支持资产推送：
   - 使用 WS user channel 接收账户、订单、fill 增量；
   - 使用 REST 在启动和断线重连后做资产/订单快照校准。
3. 将订单状态和资产状态统一转换为 `ExchangeEvent`，写入环形队列 2。

重要去重规则：

- Order thread 和 account thread 都可能接收到订单状态。
- 必须通过 `exchange_order_id`、`client_order_id`、`exec_id`、`sequence` 做幂等去重。
- 对策略只暴露一次规范化后的订单生命周期事件。

#### D. Supervisor / heartbeat thread（可选，但建议）

职责：

- 管理线程生命周期。
- 监控 FIX heartbeat / test request / sequence reset。
- 采集延迟指标、drop 计数、重连次数。
- 控制优雅停机。

---

## 3. 建议代码结构

当前项目很小：

```text
cb-hft/
├── Cargo.toml
├── src/
│   └── main.rs
└── docs/
    └── coinbase-hft-architecture.md
```

建议扩展为：

```text
cb-hft/
├── Cargo.toml
├── config/
│   ├── prod.toml.example
│   └── sandbox.toml.example
├── docs/
│   └── coinbase-hft-architecture.md
├── benches/
│   ├── fix_parse_bench.rs
│   └── market_state_bench.rs
├── tests/
│   ├── fix_parser_tests.rs
│   ├── order_lifecycle_tests.rs
│   └── market_state_tests.rs
└── src/
    ├── main.rs
    ├── app.rs
    ├── config.rs
    ├── time.rs
    ├── error.rs
    ├── ids.rs
    ├── types/
    │   ├── mod.rs
    │   ├── symbol.rs
    │   ├── price.rs
    │   ├── qty.rs
    │   ├── side.rs
    │   ├── order.rs
    │   └── account.rs
    ├── cpu/
    │   ├── mod.rs
    │   └── affinity.rs
    ├── ring/
    │   ├── mod.rs
    │   ├── command.rs
    │   └── event.rs
    ├── fix/
    │   ├── mod.rs
    │   ├── codec.rs
    │   ├── parser.rs
    │   ├── encoder.rs
    │   ├── session.rs
    │   ├── tags.rs
    │   ├── messages.rs
    │   ├── checksum.rs
    │   └── coinbase/
    │       ├── mod.rs
    │       ├── constants.rs
    │       ├── logon.rs
    │       ├── market_data.rs
    │       ├── order_entry.rs
    │       └── account.rs
    ├── net/
    │   ├── mod.rs
    │   ├── tcp.rs
    │   ├── tls.rs
    │   └── socket_tuning.rs
    ├── market/
    │   ├── mod.rs
    │   ├── l1_book.rs
    │   ├── trade.rs
    │   ├── matcher.rs
    │   ├── state.rs
    │   └── thread.rs
    ├── order/
    │   ├── mod.rs
    │   ├── command.rs
    │   ├── manager.rs
    │   ├── lifecycle.rs
    │   ├── fix_client.rs
    │   └── thread.rs
    ├── account/
    │   ├── mod.rs
    │   ├── balances.rs
    │   ├── ws_client.rs
    │   ├── rest_client.rs
    │   └── thread.rs
    ├── strategy/
    │   ├── mod.rs
    │   ├── api.rs
    │   └── noop.rs
    ├── telemetry/
    │   ├── mod.rs
    │   ├── metrics.rs
    │   ├── latency.rs
    │   └── logging.rs
    └── supervisor/
        ├── mod.rs
        └── shutdown.rs
```

### 3.1 模块职责

#### `types/`

全局领域类型，禁止在热路径中使用 `String` / `f64`。

- `SymbolId`：内部 symbol 整数 ID。
- `Product`：Coinbase symbol，例如 `BTC-USD`。
- `Price`：定点整数价格。
- `Qty`：定点整数数量。
- `OrderId` / `ClientOrderId` / `ExchangeOrderId`。
- `Side`：Buy / Sell。
- `TimeNs`：纳秒时间戳。

#### `fix/`

通用 FIX 引擎，不绑定 Coinbase 业务。

- `codec.rs`：TCP byte stream -> complete FIX frame；处理粘包/半包。
- `parser.rs`：零分配 tag=value parser。
- `encoder.rs`：预分配 buffer 写 FIX message。
- `session.rs`：FIX session state，维护 sender/target comp id、seq num、heartbeat、test request、resend request、sequence reset。
- `checksum.rs`：BodyLength(9) / CheckSum(10) 计算。
- `messages.rs`：通用 FIX typed message。

#### `fix/coinbase/`

Coinbase 方言与消息映射。

- `constants.rs`：Coinbase endpoint、FIX version、required tags、MsgType、MDEntryType 等。
- `logon.rs`：Coinbase FIX logon 签名、认证字段、session 初始化。
- `market_data.rs`：MarketDataRequest、MarketDataSnapshotFullRefresh、MarketDataIncrementalRefresh -> `MarketEvent`。
- `order_entry.rs`：NewOrderSingle、Cancel、ExecutionReport -> `OrderEvent`。
- `account.rs`：如果 FIX 支持账户/资产类消息，则在这里映射；否则只保留接口。

#### `market/`

symbol-local 热路径。

- `l1_book.rs`：本地 best bid/ask。
- `trade.rs`：逐笔成交事件。
- `state.rs`：MarketState，聚合 L1、last trade、订单生命周期镜像、资产快照。
- `matcher.rs`：将行情事件 + 当前策略订单状态撮合为策略输入。
- `thread.rs`：market thread 主循环。

#### `order/`

下单、撤单、订单生命周期。

- `command.rs`：策略命令模型。
- `manager.rs`：client order id、pending order map、幂等去重、风险检查。
- `lifecycle.rs`：New -> PendingNew -> Open -> PartiallyFilled -> Filled/Canceled/Rejected。
- `fix_client.rs`：Order Entry FIX session wrapper。
- `thread.rs`：order thread 主循环。

#### `ring/`

环形队列封装。

- `command.rs`：market threads -> order thread。
- `event.rs`：order/account threads -> market threads。

由于有多个 market thread 生产到一个 order thread，环形队列 1 有两个实现选项：

1. **每个 market thread 一个 SPSC ring 到 order thread**：推荐，最低延迟，避免 MPSC CAS 竞争。
2. 一个全局 MPSC ring：实现简单，但在多个 symbol 同时活跃时 CAS 竞争更明显。

环形队列 2 有两个实现选项：

1. **每个 market thread 一个 SPSC event ring**：推荐。order/account thread 根据 `symbol_id` 路由事件到对应 ring。
2. 一个广播 ring：实现复杂，并且每个 market thread 需要过滤 symbol。

建议首版采用：

```text
cmd rings:   market[i] ──SPSC──> order thread
event rings: order/account ──SPSC per symbol──> market[i]
```

注意：account thread 和 order thread 都会写 event ring，所以 event ring 若严格 SPSC，需要再做一层 event router thread，或使用每个 market 两条 SPSC event ring：

```text
order_event_ring[i]:   order thread   ──SPSC──> market[i]
account_event_ring[i]: account thread ──SPSC──> market[i]
```

为了保持低延迟和简单性，建议使用“两条 SPSC event ring”。

#### `strategy/`

策略接口，不实现具体策略。

- `api.rs`：定义 `Strategy` trait。
- `noop.rs`：默认空策略，用于集成测试。

---

## 4. 核心数据结构

### 4.1 定点数

```rust
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Price(pub i64); // scaled integer, e.g. quote currency atomic unit

#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Qty(pub i64); // scaled integer, e.g. base currency atomic unit
```

建议为每个 product 配置：

```rust
pub struct ProductSpec {
    pub symbol_id: SymbolId,
    pub coinbase_product: &'static str, // "BTC-USD"
    pub price_scale: i64,
    pub qty_scale: i64,
    pub min_qty: Qty,
    pub min_notional: i64,
    pub price_tick: Price,
    pub qty_step: Qty,
}
```

FIX 中价格/数量是 ASCII decimal，应使用自定义 parser 直接解析到 scaled integer，避免 `f64`。

### 4.2 Symbol

```rust
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SymbolId(pub u16);

pub struct SymbolRegistry {
    // 启动时构造，热路径只用 SymbolId，不用 String 查找。
    pub products: &'static [ProductSpec],
}
```

### 4.3 MarketEvent

```rust
#[derive(Clone, Copy, Debug)]
pub enum MarketEvent {
    L1(L1Update),
    Trade(Trade),
    Session(SessionEvent),
}

#[derive(Clone, Copy, Debug)]
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

#[derive(Clone, Copy, Debug)]
pub struct Trade {
    pub symbol_id: SymbolId,
    pub exchange_ts_ns: u64,
    pub recv_ts_ns: u64,
    pub trade_id: u64,
    pub price: Price,
    pub qty: Qty,
    pub aggressor_side: Option<Side>,
    pub sequence: u64,
}
```

### 4.4 L1Book

```rust
#[derive(Clone, Copy, Debug, Default)]
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
```

### 4.5 StrategyCommand（环形队列 1）

```rust
#[derive(Clone, Copy, Debug)]
pub enum StrategyCommand {
    NewOrder(NewOrderCommand),
    CancelOrder(CancelOrderCommand),
    ReplaceOrder(ReplaceOrderCommand),
    CancelAll(CancelAllCommand),
}

#[derive(Clone, Copy, Debug)]
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

#[derive(Clone, Copy, Debug)]
pub struct CancelOrderCommand {
    pub symbol_id: SymbolId,
    pub client_order_id: ClientOrderId,
    pub strategy_order_id: u64,
    pub signal_ts_ns: u64,
}
```

### 4.6 ExchangeEvent（环形队列 2）

```rust
#[derive(Clone, Copy, Debug)]
pub enum ExchangeEvent {
    Order(OrderEvent),
    Fill(FillEvent),
    Balance(BalanceEvent),
    Session(SessionEvent),
    Risk(RiskEvent),
}
```

### 4.7 OrderEvent

```rust
#[derive(Clone, Copy, Debug)]
pub struct OrderEvent {
    pub symbol_id: SymbolId,
    pub client_order_id: ClientOrderId,
    pub exchange_order_id: ExchangeOrderId,
    pub status: OrderStatus,
    pub side: Side,
    pub price: Price,
    pub original_qty: Qty,
    pub remaining_qty: Qty,
    pub filled_qty: Qty,
    pub avg_fill_px: Price,
    pub event_ts_ns: u64,
    pub recv_ts_ns: u64,
    pub source: OrderEventSource,
    pub dedup_key: DedupKey,
}
```

### 4.8 BalanceEvent

```rust
#[derive(Clone, Copy, Debug)]
pub struct BalanceEvent {
    pub asset_id: AssetId,
    pub total: Qty,
    pub available: Qty,
    pub hold: Qty,
    pub update_ts_ns: u64,
    pub recv_ts_ns: u64,
}
```

---

## 5. FIX 设计

### 5.1 FIX parser

目标：从 TCP stream 中找出完整 FIX message，并将 tag/value 映射成 typed event。

设计要点：

- FIX delimiter 使用 SOH (`0x01`)。
- 按 `8=...`、`9=BodyLength`、`35=MsgType`、`10=CheckSum` 识别 message 边界。
- Parser 不分配内存，返回借用输入 buffer 的 `FixField<'a>`。
- 关键 tag 使用整数比较，不转字符串。
- 对热路径消息只解析需要字段；非关键字段跳过。
- 校验 `BodyLength` 和 `CheckSum`，但可配置在 benchmark 中测量校验成本；生产默认开启。

核心接口建议：

```rust
pub struct FixFrame<'a> {
    pub raw: &'a [u8],
    pub body: &'a [u8],
    pub msg_type: MsgType,
}

pub struct FixField<'a> {
    pub tag: u32,
    pub value: &'a [u8],
}

pub struct FixParser;

impl FixParser {
    pub fn next_frame<'a>(&mut self, buf: &'a [u8]) -> Result<Option<(FixFrame<'a>, usize)>, FixError>;
    pub fn fields<'a>(&self, frame: &'a FixFrame<'a>) -> FixFieldIter<'a>;
}
```

### 5.2 FIX session

`FixSession` 负责：

- logon / logout。
- heartbeat。
- test request。
- sender sequence / target sequence。
- resend request / sequence reset。
- reject / business reject 处理。
- reconnect 后状态恢复。

建议不要在第一版实现完整通用 FIX engine 的所有边界，而是覆盖 Coinbase 必需路径，并用集成测试逐步扩展。

### 5.3 Coinbase FIX Market Data

需要实现：

- 登录 Market Data FIX session。
- 发送 MarketDataRequest：订阅每个 product 的 trade 与 L1 quote。
- 解析：
  - snapshot / incremental refresh。
  - best bid / best offer。
  - trade print。
  - sequence / timestamp。
- 断线后：
  - 停止策略发单或切换为只撤单模式。
  - 重连。
  - 重新订阅。
  - 重新初始化 L1 state。

待确认：

- Coinbase FIX Market Data 是否允许一个 session 订阅多个 symbols，以及 session 数限制。
- L1 数据在 FIX 中是 snapshot full refresh 还是 incremental refresh。
- 逐笔成交是否带 aggressor side、trade id、exchange sequence。
- 是否需要从 WS `matches` 或 `ticker` 补充字段。

### 5.4 Coinbase FIX Order Entry

需要实现：

- 登录 Order Entry FIX session。
- 编码并发送：
  - NewOrderSingle。
  - OrderCancelRequest。
  - 可选：OrderCancelReplaceRequest。
- 解析：
  - ExecutionReport。
  - OrderCancelReject。
  - Reject / BusinessMessageReject。
- 本地维护：
  - client order id -> internal order state。
  - exchange order id -> client order id。
  - pending cancel / pending replace。
  - fill dedup。

下单线程应具备基本风控：

- 单 symbol 最大未完成订单数。
- 单 symbol 最大名义敞口。
- 每秒最大 new/cancel 请求数。
- 最小下单量、价格 tick、数量 step 校验。
- post-only 默认开启，避免无意 taker。
- 行情断线、资产断线、订单回报断线时的降级策略。

### 5.5 资产与订单推送

订单状态：优先从 Order Entry FIX ExecutionReport 获取。

资产状态：需要确认 Coinbase FIX 是否提供资产/余额推送。建议设计为：

1. 启动时 REST 拉取账户余额快照。
2. 运行中如果 FIX 无资产推送，则使用 WS user channel 接收余额/订单/fill 增量。
3. 定时 REST reconcile，避免 WS 丢包或解析遗漏。
4. 所有来源统一映射为 `BalanceEvent` / `OrderEvent`。

---

## 6. 线程与 CPU 绑定

### 6.1 CPU core 分配

配置示例：

```toml
[threading]
order_core = 2
account_core = 3
supervisor_core = 4

[[products]]
symbol = "BTC-USD"
market_core = 5

[[products]]
symbol = "ETH-USD"
market_core = 6

[[products]]
symbol = "SOL-USD"
market_core = 7
```

### 6.2 Rust 实现建议

- macOS：使用 `thread_policy_set` / `pthread` 相关 API 的可用封装；但 macOS 对硬绑定支持不如 Linux，更多是 affinity tag / QoS hint。
- Linux 生产环境：使用 `core_affinity` 或 `libc::sched_setaffinity` 做真实 core pinning。
- 建议代码抽象：

```rust
pub trait CpuAffinity {
    fn pin_current_thread(core_id: usize) -> Result<(), AffinityError>;
}
```

`cpu/affinity.rs` 根据 target OS 编译不同实现。

### 6.3 运行环境建议

若追求真实 HFT 延迟，应在 Linux 裸机部署，并考虑：

- CPU isolation：`isolcpus`, `nohz_full`, `rcu_nocbs`。
- 禁用 CPU frequency scaling / turbo 策略固定。
- NIC RSS/RPS/XPS 与 thread core 绑定。
- `SO_BUSY_POLL`、socket buffer 调优。
- 进程优先级与 `mlockall`。
- 日志异步化，热路径不落盘。

---

## 7. Ring buffer 方案

### 7.1 推荐 crate

优先选择：

- `rtrb`：SPSC ring buffer，适合低延迟。
- 或 `crossbeam-channel`：简单可靠，但延迟不如专用 ring。
- 或自研固定容量 SPSC ring：后续若 benchmark 显示 crate 成本过高再做。

首版建议：`rtrb`。

### 7.2 队列拓扑

```text
market[i] -> order:
  cmd_ring[i]: Producer<StrategyCommand> in market thread
               Consumer<StrategyCommand> in order thread

order -> market[i]:
  order_event_ring[i]: Producer<ExchangeEvent> in order thread
                       Consumer<ExchangeEvent> in market thread

account -> market[i]:
  account_event_ring[i]: Producer<ExchangeEvent> in account thread
                         Consumer<ExchangeEvent> in market thread
```

### 7.3 满队列策略

- `StrategyCommand` ring 满：
  - 不应阻塞 market thread 太久。
  - 计数并触发风控降级。
  - 对 cancel 命令优先级应高于 new order。
  - 可设计两个 ring：`cancel_priority_ring` 和 `new_order_ring`。
- `ExchangeEvent` ring 满：
  - 这是严重错误；订单生命周期不能丢。
  - 应进入 fail-safe：暂停发新单，尝试撤单，报警。

---

## 8. Strategy 接口预留

```rust
pub trait Strategy: Send + 'static {
    fn on_l1(&mut self, ctx: &mut StrategyContext, book: &L1Book);
    fn on_trade(&mut self, ctx: &mut StrategyContext, trade: &Trade);
    fn on_order_event(&mut self, ctx: &mut StrategyContext, event: &OrderEvent);
    fn on_balance_event(&mut self, ctx: &mut StrategyContext, event: &BalanceEvent);
}

pub struct StrategyContext<'a> {
    pub symbol_id: SymbolId,
    pub now_ns: u64,
    pub cmd_producer: &'a mut CommandProducer,
    pub market_state: &'a MarketState,
}
```

首版提供 `NoopStrategy`：只消费事件，不发单，用于验证行情和订单回报链路。

---

## 9. 配置设计

建议 `config/prod.toml.example`：

```toml
[coinbase]
environment = "prod" # prod | sandbox
api_key_env = "COINBASE_API_KEY"
api_secret_env = "COINBASE_API_SECRET"
passphrase_env = "COINBASE_PASSPHRASE"

[coinbase.fix.market_data]
# endpoint 以 Coinbase 官方文档为准，代码中不要硬编码到热路径
host = "FIX_MD_HOST_FROM_DOCS"
port = 0
heartbeat_secs = 30

[coinbase.fix.order_entry]
host = "FIX_OE_HOST_FROM_DOCS"
port = 0
heartbeat_secs = 30

[coinbase.ws]
user_feed_url = "WSS_USER_FEED_URL_FROM_DOCS"

[risk]
default_post_only = true
max_open_orders_per_symbol = 20
max_order_rate_per_sec = 50
max_cancel_rate_per_sec = 100

[threading]
order_core = 2
account_core = 3
supervisor_core = 4

[ring]
cmd_capacity = 4096
order_event_capacity = 8192
account_event_capacity = 8192

[[products]]
symbol = "BTC-USD"
market_core = 5
price_scale = 100
qty_scale = 100000000
price_tick = "0.01"
qty_step = "0.00000001"

[[products]]
symbol = "ETH-USD"
market_core = 6
price_scale = 100
qty_scale = 100000000
price_tick = "0.01"
qty_step = "0.00000001"

[[products]]
symbol = "SOL-USD"
market_core = 7
price_scale = 100
qty_scale = 100000000
price_tick = "0.01"
qty_step = "0.00000001"
```

注意：上面的 endpoint 占位符必须在实现时从 Coinbase 最新官方文档核对，不应凭记忆写死。

---

## 10. Cargo 依赖建议

初始依赖建议：

```toml
[dependencies]
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
rtrb = "0.3"
bytes = "1"
socket2 = "0.5"
rustls = "0.23"
rustls-native-certs = "0.8"
webpki-roots = "0.26"
core_affinity = "0.8"
arrayvec = "0.7"
smallvec = "1"

[dev-dependencies]
criterion = "0.5"
proptest = "1"
```

依赖原则：

- 热路径不使用 `serde`；`serde` 仅用于配置和非热路径 JSON/REST。
- 如果 WS user feed 需要 JSON，可在 account thread 使用 `serde_json`，但不能进入 market parse 热路径。
- FIX parser/encoder 自研，避免通用 FIX crate 的 allocation 和动态 dispatch。

---

## 11. 测试与性能验证

### 11.1 单元测试

- `fix_parser_tests.rs`
  - 完整 message 解析。
  - 半包/粘包处理。
  - BodyLength 错误。
  - CheckSum 错误。
  - decimal ASCII -> scaled integer。
- `market_state_tests.rs`
  - L1 update 顺序应用。
  - sequence 回退/重复忽略。
  - trade event 更新。
- `order_lifecycle_tests.rs`
  - New -> Open。
  - New -> Rejected。
  - Open -> PartiallyFilled -> Filled。
  - Open -> PendingCancel -> Canceled。
  - cancel reject。
  - duplicate exec report 去重。

### 11.2 集成测试

- 使用 fixture FIX messages 回放。
- 模拟 market thread 产生 signal。
- 模拟 order thread 回报。
- 验证 strategy lifecycle 状态一致。

### 11.3 Benchmark

`benches/fix_parse_bench.rs`：

- 单条 L1 message parse。
- 单条 trade message parse。
- 粘包中连续 N 条 message parse。
- checksum on/off 对比。

`benches/market_state_bench.rs`：

- parse + typed event + L1 apply。
- parse + trade apply。
- parse + strategy no-op callback。

目标输出：

- mean / p50 / p95 / p99。
- allocation count 必须为 0。
- p99 < 2µs 才算达标。

建议后续引入：

- `cargo bench`。
- `heaptrack` / `dhat` / `valgrind massif`（Linux）。
- `perf stat` / `perf record`（Linux）。
- `criterion` + 固定 CPU 频率。

---

## 12. 实施阶段建议

### Phase 0：核对 Coinbase 官方协议

交付物：`docs/coinbase-fix-notes.md`

需要确认：

- FIX Market Data endpoint、Order Entry endpoint、sandbox endpoint。
- FIX version。
- 登录签名算法与 required tags。
- MarketDataRequest 订阅 L1/trade 的准确 tags。
- MarketDataSnapshot / IncrementalRefresh 的字段含义。
- ExecutionReport 字段映射。
- 是否有 FIX 资产推送。
- WS user channel 的鉴权、sequence、balance/order/fill 事件格式。

### Phase 1：基础类型、配置、线程启动

- 建立 `types/`。
- 建立 `config.rs`。
- 建立 CPU affinity 抽象。
- 建立 ring buffer 拓扑。
- 启动 market/order/account/noop threads，但不连接交易所。

### Phase 2：FIX parser/encoder/session

- 实现 FIX frame codec。
- 实现 zero-allocation parser。
- 实现 checksum/body length。
- 实现 encoder。
- 实现 session heartbeat/logon/logout。
- 用 fixture 测试。

### Phase 3：Market Data FIX

- 实现 Coinbase MarketDataRequest。
- 接入 sandbox 或 prod read-only 行情。
- 更新 L1Book 和 Trade。
- NoopStrategy 验证事件流。
- benchmark parse + L1 apply。

### Phase 4：Order Entry FIX

- 实现 NewOrderSingle / Cancel。
- 实现 ExecutionReport 映射。
- 实现本地订单生命周期。
- 先在 sandbox 跑最小数量 post-only 订单；如果没有 sandbox，必须 dry-run/replay 通过后再人工确认 prod。

### Phase 5：Account / User Feed

- FIX 支持则接 FIX。
- 否则接 WS user channel + REST snapshot。
- 实现 balance/order/fill 去重与 reconcile。

### Phase 6：Fail-safe 与恢复

- 行情断线：暂停发新单。
- order session 断线：禁止策略继续发单，必要时恢复后撤单。
- account session 断线：降低风险额度或暂停。
- sequence gap：进入 resync。
- ring 满：报警 + fail-safe。

### Phase 7：延迟优化

- fixture benchmark 达标后再连接实盘。
- 优化 decimal parser、tag dispatch、branch prediction。
- 如果 `rtrb` 不达标，再考虑自研 ring。
- 如果 TLS/read 开销过高，拆分 read/decode 或使用更底层 socket tuning。

---

## 13. 关键设计取舍

### 13.1 每 symbol 一个 MD TCP 连接 vs 一个连接订阅多 symbol

推荐首选：每 symbol 一个 MD TCP 连接。

优点：

- symbol 间隔离好。
- 单线程本地 state 简单。
- 延迟抖动更可控。
- 不需要跨线程分发行情。

缺点：

- 连接数更多。
- Coinbase session limit 可能不允许。
- 登录/重连管理更复杂。

需要保留 fallback：单 MD session 订阅多 symbol，然后 dispatcher 分发到 per-symbol ring。

### 13.2 SPSC 多队列 vs MPSC 单队列

推荐：SPSC 多队列。

理由：

- 延迟更稳定。
- 避免 MPSC 原子竞争。
- 交易对数量小（<=10），order thread 轮询 10 个 ring 成本可控。

### 13.3 blocking thread vs async runtime

推荐：blocking dedicated threads。

理由：

- 更容易做 CPU pinning。
- 更容易控制延迟和调度。
- 避免 async runtime wakeup、task migration、动态调度抖动。

非热路径 WS/REST 可以使用 async，但建议隔离在 account thread 或单独 runtime 中。

### 13.4 通用 FIX crate vs 自研 FIX hot path

推荐：自研轻量 FIX parser/encoder。

理由：

- 2µs 目标下，通用 FIX crate 往往有过多 abstraction/allocation。
- Coinbase 需要的消息集合有限。
- 自研可以只解析必要字段。

---

## 14. 风险与待确认问题

### 14.1 需要你确认的问题

1. **部署环境**：最终运行在 macOS 还是 Linux 裸机？如果是 macOS，CPU 绑定和网络延迟优化能力有限，2µs 只能作为用户态解析/撮合 benchmark 目标。
2. **Coinbase 产品线**：使用 Coinbase Exchange/Advanced Trade 哪套 API？FIX 文档和鉴权字段需要精确对应。
3. **环境**：是否有 sandbox 账号？是否允许先连 sandbox 做下单/撤单闭环？
4. **行情源**：逐笔成交和 L1 是否必须都来自 FIX？如果 FIX trade 字段不完整，是否允许 WS 补充？
5. **资产推送**：若 FIX 无资产推送，是否接受 REST snapshot + WS user feed 的混合方案？
6. **订单类型**：首版是否只支持 limit + post-only + cancel？是否需要 replace/amend？
7. **精度配置**：每个产品的 tick size、base increment、quote increment 是固定配置，还是启动时从 Coinbase REST product endpoint 拉取？
8. **风控策略**：最大订单数、最大仓位、最大撤单率、断线后的动作需要你给具体阈值。
9. **日志要求**：是否需要全量 FIX 原始报文审计落盘？如果需要，必须异步写，不能在热路径同步写。
10. **2µs 口径**：是否认可“从用户态 buffer 中一条完整 FIX message 开始，到 typed event + L1 apply 完成”为验收口径？

### 14.2 技术风险

- Coinbase FIX session / connection limit 影响“每交易对一个连接”的设计。
- TLS 与网络 read 开销远超 2µs；必须分清协议解析目标与端到端目标。
- 资产推送可能不能走 FIX，需要 WS/REST 混合。
- FIX sequence gap 和 reconnect 处理复杂，必须优先做 fail-safe。
- 如果订单回报来自两个来源，去重错误会直接影响策略生命周期。

---

## 15. 首版最小可交付范围（MVP）

建议第一版只做：

- 3 个 symbol：BTC-USD、ETH-USD、SOL-USD。
- 每 symbol 一个 market thread。
- order thread 一个。
- account thread 一个。
- 策略使用 `NoopStrategy` 或只发 dry-run command。
- FIX parser/encoder/session 基础能力。
- Market Data 行情接入 + L1/trade 更新。
- Order Entry sandbox 下单/撤单闭环。
- WS/REST 资产快照 fallback。
- Criterion benchmark：parse + L1 apply p99 < 2µs。

不建议首版做：

- 多交易所抽象。
- 深度 L2/L3 order book。
- 复杂策略框架。
- 持久化数据库。
- GUI/dashboard。
- 通用 FIX 全协议覆盖。

---

## 16. 下一步

如果你认可这个方案，我建议下一步按以下顺序推进：

1. 先补充 `docs/coinbase-fix-notes.md`，逐项核对 Coinbase 官方 FIX/WS 文档。
2. 修改 `Cargo.toml`，加入基础依赖。
3. 创建 `types/`、`ring/`、`strategy/`、`market/` 骨架。
4. 用 fixture 先实现 FIX parser benchmark，确认 2µs 目标可达。
5. 再接真实 Coinbase Market Data。
6. 最后接 Order Entry 和 Account/User Feed。
