# cb-hft

`cb-hft` 是一个面向 Coinbase Exchange 的 Rust 交易所接入项目。当前仓库重点是把行情、订单/成交用户推送、资产快照等交易所交互层先跑通；策略部分暂时留空，目前运行时只打印接收到的数据，不会自动发单。

## 当前能力

已实现：

- Coinbase WebSocket 公共行情接入：
  - `ticker`：打印 L1 bid/ask、最新成交价、时间和 sequence。
  - `matches`：打印逐笔成交。
- Coinbase WebSocket 认证行情/用户推送接入：
  - 配置 API 凭据后，行情订阅会尝试启用 `level2` 深度。
  - 配置 API 凭据后，会订阅 `user` / `full` 用户通道，打印订单和成交事件。
- Coinbase REST 资产快照：
  - 配置 API 凭据后，启动时请求 `GET /accounts` 并打印资产余额、可用余额、冻结余额。
- 本地协议/领域模型：
  - FIX parser / encoder。
  - Coinbase FIX logon 签名。
  - Coinbase Market Data fixture 解析。
  - ExecutionReport fixture 解析。
  - OrderManager 基础订单生命周期和风控检查。
  - SPSC ring topology 骨架。
  - Noop strategy / strategy trait 骨架。

当前不会自动下单。`order` / `fix` 模块里已有 FIX NewOrderSingle、CancelRequest 编码和 ExecutionReport 解析基础，但真实 Order Entry FIX 网络 session 尚未作为默认运行路径接入。

## 代码结构

```text
cb-hft/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── config/
│   ├── prod.toml.example       # 生产环境示例配置
│   └── sandbox.toml.example    # 沙盒环境示例配置
├── docs/
│   └── coinbase-hft-architecture.md
├── src/
│   ├── main.rs                 # 二进制入口，调用 runtime
│   ├── lib.rs                  # library module exports
│   ├── runtime.rs              # 当前可运行系统：WS/REST 接入和打印
│   ├── config.rs               # TOML 配置解析、产品配置解析
│   ├── types.rs                # Price/Qty/Symbol/Product 等基础类型
│   ├── event.rs                # ExchangeEvent/BalanceEvent/FillEvent 等事件类型
│   ├── market.rs               # MarketEvent、L1Book、MarketEngine
│   ├── order.rs                # StrategyCommand、OrderManager、OrderThreadEngine
│   ├── account.rs              # REST account snapshot / WS user feed JSON 解码
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
│           ├── auth.rs         # Coinbase REST/WS/FIX 签名
│           ├── market_data.rs  # FIX Market Data fixture -> MarketEvent
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
  - `prod` / `production` -> `wss://ws-feed.exchange.coinbase.com` 和 `https://api.exchange.coinbase.com`
  - 其他值默认按 sandbox 处理 -> `wss://ws-feed-public.sandbox.exchange.coinbase.com` 和 `https://api-public.sandbox.exchange.coinbase.com`
- `api_key_env` / `api_secret_env` / `passphrase_env` 是环境变量名称，不要把真实密钥写入配置文件。
- `products` 控制订阅的 Coinbase 产品列表。

## API 凭据

如果只看公共行情，不需要 API 凭据。

如果要启用资产快照、用户订单/成交推送、认证 `level2` 深度，请设置：

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

预期会打印配置、环境、产品列表，并提示 dry-run 成功。

### 2. 只运行公共行情

```bash
/Users/ml015/.cargo/bin/cargo run -- --market-data-only --config config/prod.toml.example
```

会打印类似：

```text
[market.l1] product=BTC-USD bid=... bid_size=... ask=... ask_size=... price=... time=... seq=...
[market.trade] product=BTC-USD side=... price=... size=... trade_id=... time=... seq=...
```

### 3. 公共行情 smoke test，只接收少量消息后退出

```bash
/Users/ml015/.cargo/bin/cargo run -- --market-data-only --once --config config/prod.toml.example
```

`--once` 当前会在对应线程收到少量消息后退出，适合做启动和网络连通性验证。

### 4. 启用资产和用户订单/成交 feed

先设置 API 凭据，然后运行：

```bash
export COINBASE_API_KEY='your-api-key'
export COINBASE_API_SECRET='your-base64-secret'
export COINBASE_PASSPHRASE='your-passphrase'

/Users/ml015/.cargo/bin/cargo run -- --config config/prod.toml.example
```

启动后会：

1. 打印配置摘要。
2. 请求 REST `/accounts` 并打印资产。
3. 启动 market WebSocket 打印行情。
4. 启动 authenticated user WebSocket 打印用户订单/成交事件。

### 5. 只运行账户/用户 feed

```bash
/Users/ml015/.cargo/bin/cargo run -- --account-only --config config/prod.toml.example
```

该模式需要 API 凭据。缺少凭据时会直接报错退出。

## 命令行参数

```text
--config PATH          指定 TOML 配置文件，默认 config/sandbox.toml.example
--dry-run              只解析配置并打印摘要，不打开网络连接
--once                 收到少量消息后退出，用于 smoke test
--market-data-only     只启动行情接入，不启动 REST 资产和 user feed
--account-only         只启动 REST 资产和 user feed，不启动公共行情
--no-market-data       禁用行情接入
--no-account           禁用账户/用户 feed
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

生产公共行情 smoke test：

```bash
/Users/ml015/.cargo/bin/cargo run -- --market-data-only --once --config config/prod.toml.example
```

## 当前限制和后续 TODO

当前运行时目标是“能跑起来接数据并打印”，不是完整实盘交易系统。已知限制：

- 策略层目前留空，不会自动发单。
- WebSocket 收到的数据当前主要在 `runtime.rs` 中打印，尚未全部接入原设计里的 ring + `MarketEngine` 热路径。
- FIX Order Entry 真实 TCP/TLS session 尚未作为默认运行路径接入。
- 自动重连、sequence gap 恢复、fail-safe、断线后撤单/暂停交易等生产级逻辑仍待完善。
- `level2` 深度在 Coinbase 当前规则下需要认证；无 API 凭据时只订阅公开 `ticker` 和 `matches`。
- 延迟 benchmark 尚未补齐，当前没有验收 `parse + L1 apply p99 < 2µs`。

建议下一步：

1. 把 WebSocket/FIX 数据统一转成内部 `MarketEvent` / `ExchangeEvent`。
2. 接入 ring topology 和 `MarketEngine<NoopStrategy>`。
3. 完成 authenticated `level2` depth 的本地 book 维护。
4. 实现 Order Entry FIX 网络 session，但默认仍保持 dry-run / no-trade 安全模式。
5. 增加 reconnect/fail-safe/metrics。
6. 增加 benchmark。 
