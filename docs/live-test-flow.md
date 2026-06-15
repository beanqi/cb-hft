# cb-hft Coinbase live test flow

目标：在 API key 环境变量已设置、账户约 500U 的前提下，低风险验证 Coinbase Exchange：

- FIX Market Data 一档深度 L1 top-of-book 获取；
- FIX Market Data 逐笔成交 trade tick 获取；
- FIX Order Entry 下单与 ExecutionReport 订单推送；
- 资产/余额变动验证；
- 多笔订单、多种订单参数/格式覆盖。

## 0. 安全原则

1. 默认使用 `config/prod.toml.example` 的真实盘时，总测试名义金额控制在 50-100U 内；留足手续费和价格波动余量。
2. 每笔订单优先用 `post_only` 且远离可成交价的挂单验证格式，随后撤单；真正会成交的单只做最小数量/最小名义金额级别。
3. 每个测试批次前后都记录资产快照，测试结束必须执行 cancel-all/撤销所有活跃测试订单。
4. 测试 client_order_id 统一前缀建议：`cbhft-live-YYYYMMDD-HHMMSS-N`，便于日志筛选和应急撤单。
5. 不把 API key 写入配置或日志，只读取：
   - `COINBASE_API_KEY`
   - `COINBASE_API_SECRET`
   - `COINBASE_PASSPHRASE`

## 1. 当前代码能力与需要补齐的测试入口

当前仓库已经有：

- `--market-data-only --once`：可做 L1/trade smoke test；
- `--account-only` / `--with-account`：可做 REST `/accounts` 资产快照；
- `--order-only` / `--with-order-entry`：可以登录 Coinbase FIX Order Entry，并解析 `ExecutionReport(35=8)`；
- `OrderThreadEngine`：可以把 `StrategyCommand::NewOrder` 编码为 FIX `NewOrderSingle(35=D)`；
- live pipeline 当前默认策略是 `NoopStrategy`，不会自动发单。

因此，完整下单测试前需要增加一个“脚本化 live order test runner”，不要让普通策略自动交易。建议新增 CLI 模式：

```text
--live-order-test PLAN.toml
--live-order-test-dry-run
--max-test-notional-usd 100
--test-client-prefix cbhft-live-...
```

该 runner 做三件事：

1. 连接 FIX Order Entry，Logon 成功后按 PLAN 发送订单/撤单；
2. 监听并落盘所有 `ExecutionReport`；
3. 每个阶段前后调用 REST `/accounts`，生成余额 diff。

## 2. 测试前置检查

### 2.1 编译与离线测试

```bash
cd /Users/ml015/code/rust/cb-hft
/Users/ml015/.cargo/bin/cargo fmt -- --check
/Users/ml015/.cargo/bin/cargo test
/Users/ml015/.cargo/bin/cargo build --release
```

验收：全部通过。

### 2.2 配置 dry-run

```bash
/Users/ml015/.cargo/bin/cargo run -- --dry-run --config config/prod.toml.example
```

验收：输出中应包含：

```text
[runtime] environment=prod
[runtime] products=BTC-USD,ETH-USD,SOL-USD
[runtime] fix_market_data=true ... dry_run=true
```

### 2.3 凭据与资产快照

```bash
/Users/ml015/.cargo/bin/cargo run -- --account-only --config config/prod.toml.example
```

验收：看到 `[account.balance]`，至少有 USD/USDC 相关可用余额；余额足够覆盖测试预算。

## 3. 行情测试：L1 一档深度 + 逐笔成交

### 3.1 smoke test

```bash
/Users/ml015/.cargo/bin/cargo run --release -- --market-data-only --once --config config/prod.toml.example
```

验收：

- 收到 FIX Logon：`[market.fix] received Logon 35=A`；
- 发送订阅：`MarketDataRequest 35=V 263=1 264=1`；
- 至少出现 1 条 L1：`[market.fix.l1]`；
- 至少出现 1 条 trade：`[market.fix.trade]`。

### 3.2 连续采样

建议运行 2-5 分钟并落盘：

```bash
/Users/ml015/.cargo/bin/cargo run --release -- --market-data-only --config config/prod.toml.example \
  2>&1 | tee data/live-md-$(date +%Y%m%d-%H%M%S).log
```

验收：

- 每个配置产品都有 L1；
- 活跃产品如 BTC-USD/ETH-USD 有 trade tick；
- `seq` 单调或无明显倒退；
- bid < ask，qty > 0；
- 没有 checksum/body length/parser 错误。

## 4. 订单入口测试：Logon + 空跑

```bash
/Users/ml015/.cargo/bin/cargo run --release -- --order-entry-only --once --config config/prod.toml.example
```

验收：

- 连接 `fix-ord.exchange.coinbase.com:6121`；
- 发送 `TargetCompID=CBSE` 的 Logon；
- 收到 `35=A` 后退出；
- 无订单发送。

## 5. 多笔订单测试矩阵

账户只有 500U，建议只选一个高流动性产品先跑，例如 `BTC-USD` 或 `ETH-USD`。数量用产品最小名义金额略上方，例如 10-15U/笔。每组最多 1-2 笔，批次间确认资产和挂单状态。

### 5.1 不成交格式验证：post-only buy/sell + cancel

目的：验证 `NewOrderSingle(35=D)` 编码、订单 accepted/open 推送、cancel 推送，不产生真实成交。

订单：

- BUY post_only limit，价格 = 当前 best_bid 下方较远，例如 `best_bid * 0.95`；
- SELL post_only limit，价格 = 当前 best_ask 上方较远，例如 `best_ask * 1.05`；
- 数量：约 10-15U 名义金额；
- 等待 `New/Open` ExecutionReport；
- 随后发送 CancelRequest，并等待 `Canceled` ExecutionReport。

验收：

- 每笔订单至少收到 New/Open 类状态；
- cancel 后收到 Canceled；
- `ExecID(17)` 去重生效；
- 资产 hold 在挂单后增加、撤单后释放；
- 无成交或 last_qty=0。

### 5.2 真实小额成交：taker buy + taker sell

目的：验证 fill 推送、成交字段、余额变动。

订单：

- 小额 BUY limit，价格穿过 ask，例如 `ask * 1.001`，不设置 post_only；
- 收到 full/partial fill 后，用实际买到数量发 SELL limit，价格穿过 bid，例如 `bid * 0.999`；
- 每边名义金额控制 10-15U。

验收：

- 收到 `ExecutionReport` 中 `LastPx(31)`、`LastQty(32)`；
- 代码打印 `[order.fix.fill]`；
- `filled_qty`、`remaining_qty`、`avg_px` 合理；
- 资产快照反映 USD/币种变化，考虑手续费后可对账；
- 测试结束币种残余尽量为 0 或低于 dust 阈值。

### 5.3 多笔并发/连续订单

目的：验证顺序号、client_order_id、open order count、推送关联。

订单：

- 连续发送 5 笔 post_only BUY，价格逐档远离 bid；
- 连续发送 5 笔 post_only SELL，价格逐档远离 ask；
- 等所有 open 后 cancel-all。

验收：

- 10 个唯一 `ClOrdID(11)`；
- FIX `MsgSeqNum(34)` 递增不重复；
- 每个订单都有对应 open/cancel 推送；
- cancel-all 后 active orders 为空；
- 不触发 `MaxOpenOrdersExceeded`，除非故意测风控。

### 5.4 各种订单格式覆盖

当前代码只真正编码了 `Limit NewOrderSingle` + 可选 `post_only`。如果要“各种格式都测试”，建议先补齐并分别做 fixture + live smoke：

- Limit GTC：普通限价挂单；
- Limit post-only：`ExecInst(18)=6`；
- Limit IOC：需要在 encoder 中明确支持 `TimeInForce(59)=3`；
- Limit FOK：需要支持 `TimeInForce(59)=4`；
- Market order：如果 Coinbase FIX OE 支持并且项目要用，需要支持 `OrdType(40)=1` 且不要带 limit price；
- CancelRequest：`35=F`；
- Replace/Modify：当前 `ReplaceOrder` 返回 Unsupported，需要先确认 Coinbase FIX 是否支持对应消息/字段，再实现。

每新增一种格式都先做离线 fixture：检查 FIX tags、BodyLength、Checksum、parser，再做 1 笔 live 小额测试。

## 6. 订单推送验收字段

每条 `ExecutionReport(35=8)` 至少检查：

- `ClOrdID(11)` 是否能关联本地订单；
- `OrderID(37)` 是否落入 event；
- `ExecID(17)` 是否唯一且去重；
- `ExecType(150)` / `OrdStatus(39)` 是否映射正确；
- `Side(54)`、`Symbol(55)` 是否正确；
- `OrderQty(38)`、`Price(44)`、`CumQty(14)`、`LeavesQty(151)`、`AvgPx(6)` 是否正确；
- 成交时 `LastPx(31)`、`LastQty(32)` 是否生成 `[order.fix.fill]`。

## 7. 资产推送/余额验证

当前项目只有 REST `/accounts` 快照，不是真正流式资产推送。建议分两层验收：

1. 现阶段先做“资产快照 diff”：
   - 测试前 `/accounts`；
   - 每次 open/fill/cancel 后 `/accounts`；
   - 对比 total/available/hold。
2. 如果必须验证“资产推送”，需要新增 Coinbase 官方支持的 authenticated account/user feed，或实现账户轮询 diff 并统一转换为 `ExchangeEvent::Balance`。

资产验收：

- post-only 挂单后 USD 或币种 hold 增加；
- 撤单后 hold 释放；
- 成交后 base/quote 余额变化与 fill 方向一致；
- 手续费差额可解释。

## 8. 日志与产物

建议每次 live test 输出到独立目录：

```text
data/live-test/YYYYMMDD-HHMMSS/
  md.log
  order.log
  account-before.json
  account-after-each-step.json
  report.json
```

最终报告包含：

- 测试环境、产品、预算；
- L1/trade 样本数量；
- 下单矩阵每个 case 的 client_order_id、结果、exec_id；
- fill 对账；
- balance diff；
- 是否有残留 open orders / dust；
- 错误和重试记录。

## 9. 建议执行顺序

1. `cargo fmt -- --check && cargo test && cargo build --release`；
2. `--dry-run`；
3. `--account-only` 初始资产快照；
4. `--market-data-only --once`；
5. 行情连续采样 2-5 分钟；
6. `--order-entry-only --once`；
7. 实现/启用 `--live-order-test-dry-run`，只打印将发送的 FIX，不联网下单；
8. 运行 post-only buy/sell + cancel；
9. 运行小额 taker buy/sell 成交；
10. 运行 5+5 多笔挂单 + cancel-all；
11. 资产快照 diff 和最终清理；
12. 汇总报告。
