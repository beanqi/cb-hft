# cb-hft

`cb-hft` 是面向 Coinbase Exchange 的 Rust FIX 接入项目。当前运行路径按官方 Coinbase Exchange **FIX Market Data 5.0** 文档接入行情：通过 TLS 连接 `fix-md` 网关，Logon 后发送 `MarketDataRequest(35=V)` 订阅 L1 一档深度和逐笔成交，并把 `35=W` / `35=X` 解析为内部 `MarketEvent`。

## 当前能力

已实现：

- Coinbase FIX Market Data 行情接入：
  - 生产：`tcp+ssl://fix-md.exchange.coinbase.com:6121`（Snapshot Enabled Gateway）
  - 沙盒：`tcp+ssl://fix-md.sandbox.exchange.coinbase.com:6121`（Snapshot Enabled Gateway）
  - Logon：`FIXT.1.1`、`TargetCompID=Coinbase`、`DefaultApplVerID(1137)=9`
  - 订阅：`35=V`、`263=1`、`264=1`，即 L1 top-of-book
  - 解析：`35=W` snapshot 和 `35=X` incremental refresh 中的 Bid/Offer/Trade
- FIX 基础协议：
  - frame parser、BodyLength、Checksum 校验
  - heartbeat / test request response
  - Coinbase FIX Logon HMAC-SHA256 base64 签名
  - MarketDataRequest 编码
  - ExecutionReport fixture 解析
- Coinbase REST 资产快照（可选）：
  - 启动参数加 `--with-account` 或 `--account-only` 时请求 `GET /accounts`
- 本地交易系统骨架：
  - `MarketEvent`、`L1Book`、`MarketEngine`
  - OrderManager 基础订单生命周期和风控检查
  - SPSC ring topology 骨架
  - Noop strategy / strategy trait 骨架

当前不会自动下单。`order` / `fix` 模块里已有 FIX NewOrderSingle、CancelRequest 编码和 ExecutionReport 解析基础，但真实 Order Entry FIX 网络 session 尚未作为默认运行路径接入。

## 代码结构

```text
cb-hft/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── config/
│   ├── prod.toml.example
│   └── sandbox.toml.example
├── docs/
│   └── coinbase-hft-architecture.md
├── src/
│   ├── main.rs                 # 二进制入口，调用 runtime
│   ├── lib.rs                  # library module exports
│   ├── runtime.rs              # FIX Market Data 运行时 + 可选 REST accounts
│   ├── config.rs               # TOML 配置解析、产品配置解析
│   ├── types.rs                # Price/Qty/Symbol/Product 等基础类型
│   ├── event.rs                # ExchangeEvent/BalanceEvent/FillEvent 等事件类型
│   ├── market.rs               # MarketEvent、L1Book、MarketEngine
│   ├── order.rs                # StrategyCommand、OrderManager、OrderThreadEngine
│   ├── account.rs              # REST account snapshot 解码
│   ├── strategy.rs             # Strategy trait、NoopStrategy、测试策略
│   ├── ring.rs                 # rtrb SPSC command/event ring 封装
│   ├── app.rs                  # AppTopology：按 symbol 创建 ring 拓扑
│   ├── cpu.rs                  # CPU affinity 抽象
│   ├── net.rs                  # socket tuning 占位/基础配置
│   ├── supervisor.rs           # shutdown signal 基础设施
│   ├── telemetry.rs            # latency sample 等基础遥测
│   └── fix/
│       ├── mod.rs
│       ├── parser.rs           # FIX frame parser、checksum/body length 校验
│       ├── encoder.rs          # FIX heartbeat/logon/MD/new/cancel 编码
│       ├── session.rs          # FIX session heartbeat/test request/sequence 基础逻辑
│       └── coinbase/
│           ├── auth.rs         # Coinbase REST/FIX 签名
│           ├── market_data.rs  # FIX Market Data -> MarketEvent
│           └── order_entry.rs  # FIX ExecutionReport -> OrderEvent
└── tests/
    ├── *_tests.rs              # parser、encoder、market、order、account、ring 等测试
```

## 环境要求

- Rust toolchain：项目使用 Rust 2024 edition。
- macOS/Linux 均可本地运行。
- 当前机器若 `cargo` 不在 PATH，可直接使用：

```bash
/Users/ml015/.cargo/bin/cargo
```

## 配置文件

默认配置文件在：

- `config/prod.toml.example`
- `config/sandbox.toml.example`

主要字段：

```toml
[coinbase]
environment = "prod" # prod | sandbox
api_key_env = "COINBASE_API_KEY"
api_secret_env = "COINBASE_API_SECRET"
passphrase_env = "COINBASE_PASSPHRASE"

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
min_qty = "0.00000001"
min_notional = 100
```

说明：

- `environment` 决定 Coinbase endpoint：
  - `prod` / `production` -> `fix-md.exchange.coinbase.com:6121` 和 `https://api.exchange.coinbase.com`
  - 其他值默认按 sandbox 处理 -> `fix-md.sandbox.exchange.coinbase.com:6121` 和 `https://api-public.sandbox.exchange.coinbase.com`
- `api_key_env` / `api_secret_env` / `passphrase_env` 是环境变量名称，不要把真实密钥写入配置文件。
- `products` 控制订阅的 Coinbase 产品列表。

## API 凭据

Coinbase FIX Market Data 文档要求使用与 FIX Order Entry 相同的认证，因此运行真实 FIX 行情需要设置：

```bash
export COINBASE_API_KEY='your-api-key'
export COINBASE_API_SECRET='your-base64-secret'
export COINBASE_PASSPHRASE='your-passphrase'
```

也可以修改配置文件里的 env 名称，让程序读取不同环境变量。

## 运行方式

先进入仓库：

```bash
cd /Users/ml015/code/rust/cb-hft
```

### 1. 配置检查，不打开网络连接

```bash
/Users/ml015/.cargo/bin/cargo run -- --dry-run --config config/sandbox.toml.example
```

### 2. 运行 FIX 行情

```bash
export COINBASE_API_KEY='your-api-key'
export COINBASE_API_SECRET='your-base64-secret'
export COINBASE_PASSPHRASE='your-passphrase'

/Users/ml015/.cargo/bin/cargo run -- --market-data-only --config config/prod.toml.example
```

会连接 FIX Market Data Snapshot Enabled Gateway，发送 Logon 和 L1 MarketDataRequest，然后打印类似：

```text
[market.fix.l1] symbol_id=0 bid_px=... bid_qty=... ask_px=... ask_qty=... seq=... recv_ts_ns=...
[market.fix.trade] symbol_id=0 trade_id=... price=... qty=... seq=... recv_ts_ns=...
```

### 3. FIX 行情 smoke test，收到少量事件后退出

```bash
/Users/ml015/.cargo/bin/cargo run -- --market-data-only --once --config config/prod.toml.example
```

### 4. 启用 REST 资产快照

```bash
/Users/ml015/.cargo/bin/cargo run -- --with-account --config config/prod.toml.example
```

## 命令行参数

```text
--config PATH          指定 TOML 配置文件，默认 config/sandbox.toml.example
--dry-run              只解析配置并打印摘要，不打开网络连接
--once                 收到少量行情事件后退出，用于 smoke test
--market-data-only     只启动 FIX 行情接入，不请求 REST assets
--account-only         只请求 REST assets，不启动 FIX 行情
--with-account         启动 FIX 行情前先请求 REST assets
--no-market-data       禁用行情接入
--no-account           禁用 REST assets
-h, --help             打印用法
```

## 测试与验证

格式化：

```bash
/Users/ml015/.cargo/bin/cargo fmt
```

运行测试：

```bash
/Users/ml015/.cargo/bin/cargo test
```

构建：

```bash
/Users/ml015/.cargo/bin/cargo build
```

## 当前限制和后续 TODO

- 策略层目前留空，不会自动发单。
- Order Entry FIX 真实 TCP/TLS session 尚未作为默认运行路径接入。
- 自动重连、sequence gap 恢复、fail-safe、断线后撤单/暂停交易等生产级逻辑仍待完善。
- 延迟 benchmark 尚未补齐，当前没有验收 `parse + L1 apply p99 < 2µs`。
