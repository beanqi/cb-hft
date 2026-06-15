use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cb_hft::account::{AccountSnapshot, parse_rest_accounts_snapshot};
use cb_hft::event::ExchangeEvent;
use cb_hft::fix::coinbase::auth::{CoinbaseAuth, CoinbaseCredentials};
use cb_hft::fix::coinbase::market_data::parse_market_data;
use cb_hft::fix::{FixEncoder, FixParser, MsgType};
use cb_hft::market::{L1Book, L1Update, MarketEvent};
use cb_hft::order::{
    NewOrderCommand, OrderEventSource, OrderManager, OrderStatus, OrderThreadAction,
    OrderThreadEngine, StrategyCommand, TimeInForce,
};
use cb_hft::types::{Price, ProductSpec, Qty, Side, SymbolId};
use native_tls::TlsConnector;
use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

const CONFIG_PATH: &str = "config/prod.toml.example";
const FIX_TIMESTAMP_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]-[hour]:[minute]:[second].[subsecond digits:3]");
const ACCOUNT_QTY_SCALE: i64 = 10_000_000_000_000_000;

fn cb_hft_command(args: &[&str]) -> (String, bool) {
    let exe = env!("CARGO_BIN_EXE_cb-hft");
    let output = Command::new(exe)
        .args(args)
        .output()
        .expect("failed to run cb-hft binary");
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (combined, output.status.success())
}

fn btc_usd_spec() -> ProductSpec {
    ProductSpec {
        symbol_id: SymbolId(0),
        coinbase_product: "BTC-USD",
        price_scale: 100,
        qty_scale: 100_000_000,
        min_qty: Qty(1),
        min_notional: 100,
        price_tick: Price(1),
        qty_step: Qty(1),
    }
}

fn live_credentials() -> CoinbaseCredentials {
    CoinbaseCredentials::new(
        std::env::var("COINBASE_API_KEY").expect("COINBASE_API_KEY must be set"),
        std::env::var("COINBASE_PASSPHRASE").expect("COINBASE_PASSPHRASE must be set"),
        std::env::var("COINBASE_API_SECRET").expect("COINBASE_API_SECRET must be set"),
    )
}

fn fix_sending_time() -> String {
    OffsetDateTime::now_utc()
        .format(FIX_TIMESTAMP_FORMAT)
        .expect("format FIX SendingTime")
}

fn timestamp_secs() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch");
    format!("{}.{:03}", now.as_secs(), now.subsec_millis())
}

fn recv_ts_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

fn account_snapshot(credentials: &CoinbaseCredentials) -> AccountSnapshot {
    let timestamp = timestamp_secs();
    let signature = CoinbaseAuth::sign_rest(credentials, &timestamp, "GET", "/accounts", "")
        .expect("sign REST accounts request");
    let response = ureq::get("https://api.exchange.coinbase.com/accounts")
        .set("CB-ACCESS-KEY", &credentials.api_key)
        .set("CB-ACCESS-SIGN", &signature)
        .set("CB-ACCESS-TIMESTAMP", &timestamp)
        .set("CB-ACCESS-PASSPHRASE", &credentials.passphrase)
        .set("User-Agent", "cb-hft-live-test/0.1")
        .call()
        .expect("GET /accounts should succeed");
    let body = response.into_string().expect("accounts response body");
    parse_rest_accounts_snapshot(body.as_bytes(), ACCOUNT_QTY_SCALE, recv_ts_ns())
        .expect("parse accounts snapshot")
}

fn available_balance(snapshot: &AccountSnapshot, asset: &str) -> Qty {
    snapshot
        .balances()
        .iter()
        .find(|balance| balance.asset_id.as_str() == asset)
        .map(|balance| balance.available)
        .unwrap_or_default()
}

fn connect_tls(host: &str) -> native_tls::TlsStream<TcpStream> {
    let tcp = TcpStream::connect((host, 6121)).expect("connect FIX TLS tcp");
    tcp.set_nodelay(true).expect("set TCP_NODELAY");
    tcp.set_read_timeout(Some(Duration::from_secs(15)))
        .expect("set read timeout");
    TlsConnector::new()
        .expect("build TLS connector")
        .connect(host, tcp)
        .expect("TLS handshake")
}

fn test_req_id<'a>(parser: &FixParser, frame: &'a cb_hft::fix::FixFrame<'a>) -> Option<&'a str> {
    parser
        .fields(frame)
        .find(|field| field.tag == 112)
        .and_then(|field| std::str::from_utf8(field.value).ok())
}

fn field_str<'a>(
    parser: &FixParser,
    frame: &'a cb_hft::fix::FixFrame<'a>,
    tag: u32,
) -> Option<&'a str> {
    parser
        .fields(frame)
        .find(|field| field.tag == tag)
        .and_then(|field| std::str::from_utf8(field.value).ok())
}

fn read_next_frame(
    stream: &mut native_tls::TlsStream<TcpStream>,
    parser: &FixParser,
    pending: &mut Vec<u8>,
) -> cb_hft::fix::FixFrame<'static> {
    let mut read_buf = [0u8; 8192];
    loop {
        if let Some((frame, consumed)) = parser.next_frame(pending).expect("parse FIX frame") {
            let owned = frame.raw.to_vec();
            pending.drain(..consumed);
            let leaked: &'static [u8] = Box::leak(owned.into_boxed_slice());
            return parser
                .next_frame(leaked)
                .expect("parse owned FIX frame")
                .expect("owned FIX frame complete")
                .0;
        }
        let n = match stream.read(&mut read_buf) {
            Ok(n) => n,
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                panic!("timed out reading FIX stream")
            }
            Err(err) => panic!("read FIX stream: {err}"),
        };
        assert!(n > 0, "FIX connection closed");
        pending.extend_from_slice(&read_buf[..n]);
    }
}

fn logon_order_entry(
    stream: &mut native_tls::TlsStream<TcpStream>,
    credentials: &CoinbaseCredentials,
) -> (FixParser, FixEncoder, u64, Vec<u8>) {
    let parser = FixParser::default();
    let encoder = FixEncoder::new("FIXT.1.1", &credentials.api_key, "CBSE");
    let mut pending = Vec::with_capacity(64 * 1024);
    let logon = encoder
        .encode_coinbase_logon(1, &fix_sending_time(), 10, credentials, true)
        .expect("encode order-entry logon");
    stream.write_all(&logon).expect("write order logon");
    stream.flush().expect("flush order logon");
    let frame = read_next_frame(stream, &parser, &mut pending);
    assert_eq!(
        frame.msg_type,
        MsgType::Logon,
        "expected order-entry logon ack"
    );
    (parser, encoder, 2, pending)
}

fn send_order_and_collect_reports(
    stream: &mut native_tls::TlsStream<TcpStream>,
    parser: &FixParser,
    encoder: &FixEncoder,
    pending: &mut Vec<u8>,
    seq: &mut u64,
    order_engine: &mut OrderThreadEngine,
    command: NewOrderCommand,
) -> Vec<ExchangeEvent> {
    let actions = order_engine.on_command(StrategyCommand::NewOrder(command), &fix_sending_time());
    assert_eq!(actions.len(), 1);
    let OrderThreadAction::SendFix(bytes) = &actions[0] else {
        panic!("order command rejected: {actions:?}");
    };
    stream.write_all(bytes).expect("write order command");
    stream.flush().expect("flush order command");
    *seq += 1;

    let deadline = SystemTime::now() + Duration::from_secs(20);
    let mut events = Vec::new();
    while SystemTime::now() < deadline {
        let frame = read_next_frame(stream, parser, pending);
        match frame.msg_type {
            MsgType::ExecutionReport => {
                let new_events = order_engine
                    .on_execution_report(parser, &frame, recv_ts_ns())
                    .expect("decode execution report");
                events.extend(new_events);
                if events.iter().any(|event| matches!(event, ExchangeEvent::Fill(_)))
                    || events.iter().any(|event| {
                        matches!(
                            event,
                            ExchangeEvent::Order(order)
                                if matches!(order.status, OrderStatus::Canceled | OrderStatus::Rejected | OrderStatus::Filled)
                        )
                    })
                {
                    return events;
                }
            }
            MsgType::TestRequest => {
                let heartbeat = encoder.encode_heartbeat(
                    *seq,
                    &fix_sending_time(),
                    test_req_id(parser, &frame),
                );
                *seq += 1;
                stream.write_all(&heartbeat).expect("write heartbeat");
                stream.flush().expect("flush heartbeat");
            }
            MsgType::Heartbeat => {}
            _ => {}
        }
    }
    panic!("timed out waiting for terminal order execution report; events={events:?}");
}

fn latest_l1_book(credentials: &CoinbaseCredentials, spec: &ProductSpec) -> L1Book {
    let mut stream = connect_tls("fix-md.exchange.coinbase.com");
    let parser = FixParser::default();
    let encoder = FixEncoder::new("FIXT.1.1", &credentials.api_key, "Coinbase");
    let mut pending = Vec::with_capacity(64 * 1024);
    let mut seq = 1u64;
    let logon = encoder
        .encode_coinbase_logon(seq, &fix_sending_time(), 10, credentials, false)
        .expect("encode market-data logon");
    seq += 1;
    stream.write_all(&logon).expect("write market logon");
    stream.flush().expect("flush market logon");
    let mut subscribed = false;
    let mut book = L1Book::default();
    let deadline = SystemTime::now() + Duration::from_secs(30);
    while SystemTime::now() < deadline {
        let frame = read_next_frame(&mut stream, &parser, &mut pending);
        match frame.msg_type {
            MsgType::Logon if !subscribed => {
                let request = encoder.encode_market_data_request_with_depth(
                    seq,
                    &fix_sending_time(),
                    "cb-hft-live-test-btc-l1",
                    1,
                    &[spec.coinbase_product],
                );
                seq += 1;
                stream
                    .write_all(&request)
                    .expect("write market data request");
                stream.flush().expect("flush market data request");
                subscribed = true;
            }
            MsgType::TestRequest => {
                let heartbeat = encoder.encode_heartbeat(
                    seq,
                    &fix_sending_time(),
                    test_req_id(&parser, &frame),
                );
                seq += 1;
                stream
                    .write_all(&heartbeat)
                    .expect("write market heartbeat");
                stream.flush().expect("flush market heartbeat");
            }
            MsgType::MarketDataSnapshotFullRefresh | MsgType::MarketDataIncrementalRefresh => {
                if field_str(&parser, &frame, 55) != Some(spec.coinbase_product) {
                    continue;
                }
                for event in parse_market_data(&parser, &frame, spec, recv_ts_ns())
                    .expect("parse market data")
                {
                    if let MarketEvent::L1 {
                        symbol_id,
                        recv_ts_ns,
                        bid_px,
                        bid_qty,
                        ask_px,
                        ask_qty,
                        sequence,
                    } = event
                    {
                        book.apply(L1Update {
                            symbol_id,
                            exchange_ts_ns: 0,
                            recv_ts_ns,
                            bid_px,
                            bid_qty,
                            ask_px,
                            ask_qty,
                            sequence,
                        });
                        if book.bid_px.0 > 0 && book.ask_px.0 > 0 {
                            return book;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    panic!("timed out waiting for BTC-USD L1 book");
}

fn qty_for_quote_notional(quote_cents: i64, price: Price, spec: &ProductSpec) -> Qty {
    let raw = ((quote_cents as i128) * (spec.qty_scale as i128) / (price.0 as i128)) as i64;
    let stepped = raw - raw % spec.qty_step.0;
    Qty(stepped.max(spec.min_qty.0))
}

fn filled_qty(events: &[ExchangeEvent]) -> Qty {
    Qty(events.iter().fold(0i64, |acc, event| match event {
        ExchangeEvent::Fill(fill) => acc + fill.qty.0,
        _ => acc,
    }))
}

#[test]
#[ignore = "requires Coinbase API env vars and live network; reads REST /accounts"]
fn live_account_snapshot_can_read_assets() {
    let (output, ok) = cb_hft_command(&["--account-only", "--config", CONFIG_PATH]);

    assert!(ok, "account snapshot command failed:\n{output}");
    assert!(
        output.contains("[account] loading REST asset snapshot"),
        "did not attempt REST account snapshot:\n{output}"
    );
    assert!(
        output.contains("[account.balance]"),
        "did not print any account balance rows:\n{output}"
    );
}

#[test]
#[ignore = "requires Coinbase API env vars and live FIX Market Data network"]
fn live_market_data_can_receive_l1_depth_and_trade_ticks() {
    let (output, ok) = cb_hft_command(&["--market-data-only", "--once", "--config", CONFIG_PATH]);

    assert!(ok, "market data command failed:\n{output}");
    assert!(
        output.contains("[market.fix] received Logon 35=A"),
        "did not receive FIX Market Data logon:\n{output}"
    );
    assert!(
        output.contains("MarketDataRequest 35=V 263=1 264=1"),
        "did not subscribe to L1 market data:\n{output}"
    );
    assert!(
        output.contains("[market.fix.l1]"),
        "did not receive any L1 depth event:\n{output}"
    );
    assert!(
        output.contains("[market.fix.trade]"),
        "did not receive any trade tick event:\n{output}"
    );
}

#[test]
#[ignore = "requires Coinbase API env vars and live FIX Order Entry network; does not place orders"]
fn live_order_entry_can_logon_without_sending_orders() {
    let (output, ok) = cb_hft_command(&["--order-entry-only", "--once", "--config", CONFIG_PATH]);

    assert!(ok, "order-entry logon command failed:\n{output}");
    assert!(
        output.contains("[order.fix] sent Logon 35=A") && output.contains("TargetCompID=CBSE"),
        "did not send Coinbase order-entry FIX logon:\n{output}"
    );
    assert!(
        output.contains("[order.fix] received Logon 35=A; order entry session ready"),
        "did not receive order-entry FIX logon ack:\n{output}"
    );
    assert!(
        !output.contains("[order.fix] sent order command"),
        "order-entry logon smoke test unexpectedly sent an order:\n{output}"
    );
}

#[test]
#[ignore = "REAL ORDERS: places live BTC-USD IOC orders using Coinbase production credentials"]
fn live_order_can_ioc_cancel_buy_then_buy_10_usd_and_sell_back() {
    let credentials = live_credentials();
    let spec = btc_usd_spec();
    let before = account_snapshot(&credentials);
    let usd_before = available_balance(&before, "USD");
    assert!(
        usd_before.0 >= 2_000 * (ACCOUNT_QTY_SCALE / 100),
        "need at least about 20 USD available for 10U buy/sell test; available scaled={}",
        usd_before.0
    );

    let mut order_stream = connect_tls("fix-ord.exchange.coinbase.com");
    let (parser, encoder, mut seq, mut pending) =
        logon_order_entry(&mut order_stream, &credentials);
    let manager = OrderManager::default();
    let mut order_engine = OrderThreadEngine::new(encoder.clone(), manager, vec![spec]);
    order_engine.set_next_seq_num(seq);

    let cancel_events = send_order_and_collect_reports(
        &mut order_stream,
        &parser,
        &encoder,
        &mut pending,
        &mut seq,
        &mut order_engine,
        NewOrderCommand {
            symbol_id: spec.symbol_id,
            side: Side::Buy,
            price: Price(500_000),
            qty: Qty(1_000_000),
            post_only: false,
            time_in_force: TimeInForce::ImmediateOrCancel,
            strategy_order_id: 1,
            signal_ts_ns: recv_ts_ns(),
        },
    );
    assert!(
        cancel_events.iter().any(|event| matches!(
            event,
            ExchangeEvent::Order(order)
                if matches!(order.status, OrderStatus::Canceled | OrderStatus::Rejected)
                    && order.filled_qty.0 == 0
        )),
        "5000 USD BTC buy IOC should cancel/reject without fill: {cancel_events:?}"
    );

    let book = latest_l1_book(&credentials, &spec);
    let buy_price = Price(book.ask_px.0 + 100);
    let buy_qty = qty_for_quote_notional(1_000, buy_price, &spec);
    assert!(buy_qty.0 > 0, "computed 10U buy qty must be positive");
    let buy_events = send_order_and_collect_reports(
        &mut order_stream,
        &parser,
        &encoder,
        &mut pending,
        &mut seq,
        &mut order_engine,
        NewOrderCommand {
            symbol_id: spec.symbol_id,
            side: Side::Buy,
            price: buy_price,
            qty: buy_qty,
            post_only: false,
            time_in_force: TimeInForce::ImmediateOrCancel,
            strategy_order_id: 2,
            signal_ts_ns: recv_ts_ns(),
        },
    );
    let bought_qty = filled_qty(&buy_events);
    assert!(
        bought_qty.0 > 0,
        "10U BTC buy should receive a fill from current ask; events={buy_events:?}"
    );

    let after_buy = account_snapshot(&credentials);
    let btc_after_buy = available_balance(&after_buy, "BTC");
    assert!(
        btc_after_buy.0 >= bought_qty.0,
        "BTC available should include bought qty; bought={} available={}",
        bought_qty.0,
        btc_after_buy.0
    );

    let sell_book = latest_l1_book(&credentials, &spec);
    let sell_price = Price((sell_book.bid_px.0 - 100).max(spec.price_tick.0));
    let sell_events = send_order_and_collect_reports(
        &mut order_stream,
        &parser,
        &encoder,
        &mut pending,
        &mut seq,
        &mut order_engine,
        NewOrderCommand {
            symbol_id: spec.symbol_id,
            side: Side::Sell,
            price: sell_price,
            qty: bought_qty,
            post_only: false,
            time_in_force: TimeInForce::ImmediateOrCancel,
            strategy_order_id: 3,
            signal_ts_ns: recv_ts_ns(),
        },
    );
    let sold_qty = filled_qty(&sell_events);
    assert!(
        sold_qty.0 > 0,
        "sell-back IOC should receive a fill; bought={} events={sell_events:?}",
        bought_qty.0
    );
    assert!(
        sold_qty.0 <= bought_qty.0,
        "sell fill should not exceed bought qty; bought={} sold={}",
        bought_qty.0,
        sold_qty.0
    );

    let after_sell = account_snapshot(&credentials);
    let usd_after = available_balance(&after_sell, "USD");
    assert!(
        usd_after.0 > 0,
        "USD balance should still be readable after round-trip; after={}",
        usd_after.0
    );
}

#[test]
fn order_feature_encodes_live_new_order_single_for_coinbase_fix() {
    let spec = btc_usd_spec();
    let encoder = FixEncoder::new("FIXT.1.1", "test-api-key", "CBSE");
    let manager = OrderManager::default();
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![spec]);
    engine.set_next_seq_num(2);

    let actions = engine.on_command(
        StrategyCommand::NewOrder(NewOrderCommand {
            symbol_id: SymbolId(0),
            side: Side::Buy,
            price: Price(1_000_000),
            qty: Qty(100_000),
            post_only: true,
            time_in_force: TimeInForce::GoodTillCancel,
            strategy_order_id: 1,
            signal_ts_ns: 123,
        }),
        "20260101-00:00:00.000",
    );

    assert_eq!(actions.len(), 1);
    let OrderThreadAction::SendFix(bytes) = &actions[0] else {
        panic!("expected order command to encode FIX NewOrderSingle, got {actions:?}");
    };
    let fix = String::from_utf8_lossy(bytes).replace('\x01', "|");

    assert!(fix.contains("35=D|"), "missing NewOrderSingle tag: {fix}");
    assert!(fix.contains("34=2|"), "wrong FIX sequence: {fix}");
    assert!(
        fix.contains("49=test-api-key|"),
        "wrong SenderCompID: {fix}"
    );
    assert!(fix.contains("56=CBSE|"), "wrong TargetCompID: {fix}");
    assert!(
        fix.contains("11=cbhft-1|"),
        "missing client order id: {fix}"
    );
    assert!(fix.contains("55=BTC-USD|"), "missing symbol: {fix}");
    assert!(fix.contains("54=1|"), "missing buy side: {fix}");
    assert!(fix.contains("40=2|"), "missing limit order type: {fix}");
    assert!(fix.contains("44=10000.00|"), "missing price: {fix}");
    assert!(fix.contains("38=0.00100000|"), "missing qty: {fix}");
    assert!(fix.contains("18=6|"), "missing post-only ExecInst: {fix}");
}

#[test]
fn order_push_feature_decodes_execution_report_into_order_and_fill_events() {
    let parser = FixParser::default();
    let spec = btc_usd_spec();
    let encoder = FixEncoder::new("FIXT.1.1", "test-api-key", "CBSE");
    let manager = OrderManager::default();
    let mut engine = OrderThreadEngine::new(encoder, manager, vec![spec]);
    let raw = b"8=FIXT.1.1\x019=201\x0135=8\x0134=3\x0149=CBSE\x0156=test-api-key\x0152=20260101-00:00:01.000\x01150=2\x0139=2\x0111=cbhft-1\x0137=order-1\x0117=exec-1\x0155=BTC-USD\x0154=1\x0144=10000.00\x0138=0.00100000\x01151=0\x0114=0.00100000\x016=10000.00\x0131=10000.00\x0132=0.00100000\x0110=220\x01";
    let (frame, consumed) = parser
        .next_frame(raw)
        .expect("valid parser result")
        .expect("complete frame");
    assert_eq!(consumed, raw.len());

    let events = engine
        .on_execution_report(&parser, &frame, 999)
        .expect("execution report should decode");

    assert_eq!(events.len(), 2, "expected order event + fill event");
    match &events[0] {
        ExchangeEvent::Order(order) => {
            assert_eq!(order.client_order_id, "cbhft-1");
            assert_eq!(order.exchange_order_id, "order-1");
            assert_eq!(order.exec_id, "exec-1");
            assert_eq!(order.status, OrderStatus::Filled);
            assert_eq!(order.source, OrderEventSource::FixOrderEntry);
            assert_eq!(order.filled_qty, Qty(100_000));
            assert_eq!(order.remaining_qty, Qty(0));
        }
        other => panic!("expected order event, got {other:?}"),
    }
    match &events[1] {
        ExchangeEvent::Fill(fill) => {
            assert_eq!(fill.client_order_id, "cbhft-1");
            assert_eq!(fill.exec_id, "exec-1");
            assert_eq!(fill.price, Price(1_000_000));
            assert_eq!(fill.qty, Qty(100_000));
        }
        other => panic!("expected fill event, got {other:?}"),
    }

    let duplicate = engine
        .on_execution_report(&parser, &frame, 1_000)
        .expect("duplicate execution report should parse");
    assert!(
        duplicate.is_empty(),
        "duplicate ExecID should not emit duplicate order/fill events"
    );
}
