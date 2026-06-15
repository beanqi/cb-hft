use std::env;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, TryRecvError},
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use native_tls::TlsConnector;
use time::{OffsetDateTime, format_description::FormatItem, macros::format_description};

use crate::account::parse_rest_accounts_snapshot;
use crate::config::{AppConfig, ProductConfig};
use crate::event::ExchangeEvent;
use crate::fix::coinbase::auth::{CoinbaseAuth, CoinbaseCredentials};
use crate::fix::coinbase::market_data::parse_market_data;
use crate::fix::{FixEncoder, FixParser, MsgType};
use crate::market::{L1Book, L1Update, MarketEngine, MarketEvent};
use crate::order::{OrderManager, OrderThreadAction, OrderThreadEngine, StrategyCommand};
use crate::strategy::{NoopStrategy, Strategy};
use crate::types::{ProductSpec, SymbolId};

const FIX_TIMESTAMP_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]-[hour]:[minute]:[second].[subsecond digits:3]");

#[derive(Clone, Debug)]
pub struct RuntimeOptions {
    pub config_path: String,
    pub dry_run: bool,
    pub market_data: bool,
    pub order_entry: bool,
    pub account: bool,
    pub once: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            config_path: "config/sandbox.toml.example".to_string(),
            dry_run: false,
            market_data: true,
            order_entry: false,
            account: false,
            once: false,
        }
    }
}

impl RuntimeOptions {
    pub fn parse_args<I, S>(args: I) -> Result<Self, RuntimeError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut opts = Self::default();
        let mut args = args.into_iter().map(Into::into).peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--config" | "-c" => {
                    opts.config_path = args.next().ok_or_else(|| {
                        RuntimeError::Usage("--config requires a path".to_string())
                    })?;
                }
                "--dry-run" => opts.dry_run = true,
                "--once" => opts.once = true,
                "--no-market-data" => opts.market_data = false,
                "--no-account" => opts.account = false,
                "--no-order-entry" => opts.order_entry = false,
                "--with-order-entry" => opts.order_entry = true,
                "--market-data-only" => {
                    opts.market_data = true;
                    opts.order_entry = false;
                    opts.account = false;
                }
                "--order-only" | "--order-entry-only" => {
                    opts.market_data = false;
                    opts.order_entry = true;
                    opts.account = false;
                }
                "--account-only" => {
                    opts.market_data = false;
                    opts.order_entry = false;
                    opts.account = true;
                }
                "--with-account" => opts.account = true,
                "--help" | "-h" => return Err(RuntimeError::Usage(usage())),
                other => {
                    return Err(RuntimeError::Usage(format!(
                        "unknown argument: {other}\n{}",
                        usage()
                    )));
                }
            }
        }
        Ok(opts)
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    Usage(String),
    Io(std::io::Error),
    Tls(native_tls::Error),
    Config(crate::config::ConfigError),
    MissingCredentials(Vec<String>),
    Http(String),
    Auth(crate::fix::coinbase::auth::CoinbaseAuthError),
    Fix(crate::fix::FixError),
    MarketData(crate::fix::coinbase::market_data::MarketDataError),
    Account(crate::account::AccountParseError),
    Time(time::error::Format),
    Protocol(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(msg) => write!(f, "{msg}"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Tls(err) => write!(f, "tls error: {err}"),
            Self::Config(err) => write!(f, "config error: {err:?}"),
            Self::MissingCredentials(names) => write!(
                f,
                "missing credential environment variables: {}",
                names.join(", ")
            ),
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Auth(err) => write!(f, "auth error: {err}"),
            Self::Fix(err) => write!(f, "fix error: {err}"),
            Self::MarketData(err) => write!(f, "market data parse error: {err:?}"),
            Self::Account(err) => write!(f, "account parse error: {err}"),
            Self::Time(err) => write!(f, "time format error: {err}"),
            Self::Protocol(err) => write!(f, "protocol error: {err}"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<std::io::Error> for RuntimeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<native_tls::Error> for RuntimeError {
    fn from(value: native_tls::Error) -> Self {
        Self::Tls(value)
    }
}

impl From<crate::config::ConfigError> for RuntimeError {
    fn from(value: crate::config::ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<crate::fix::coinbase::auth::CoinbaseAuthError> for RuntimeError {
    fn from(value: crate::fix::coinbase::auth::CoinbaseAuthError) -> Self {
        Self::Auth(value)
    }
}

impl From<crate::fix::FixError> for RuntimeError {
    fn from(value: crate::fix::FixError) -> Self {
        Self::Fix(value)
    }
}

impl From<crate::fix::coinbase::market_data::MarketDataError> for RuntimeError {
    fn from(value: crate::fix::coinbase::market_data::MarketDataError) -> Self {
        Self::MarketData(value)
    }
}

impl From<crate::account::AccountParseError> for RuntimeError {
    fn from(value: crate::account::AccountParseError) -> Self {
        Self::Account(value)
    }
}

impl From<time::error::Format> for RuntimeError {
    fn from(value: time::error::Format) -> Self {
        Self::Time(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimePipelineConfig {
    command_capacity: usize,
    exchange_event_capacity: usize,
}

impl RuntimePipelineConfig {
    pub fn new(
        _products: Vec<ProductSpec>,
        command_capacity: usize,
        exchange_event_capacity: usize,
    ) -> Result<Self, RuntimeError> {
        if command_capacity == 0 || exchange_event_capacity == 0 {
            return Err(RuntimeError::Protocol(
                "runtime pipeline ring capacities must be non-zero".to_string(),
            ));
        }
        Ok(Self {
            command_capacity,
            exchange_event_capacity,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimePipelineStep {
    MarketEventRouted { symbol_id: SymbolId },
    ExchangeEventRouted { symbol_id: Option<SymbolId> },
    StrategyCommandRouted { symbol_id: SymbolId },
    OrderAction(OrderThreadAction),
    CommandDropped { symbol_id: SymbolId },
    ExchangeEventDropped { symbol_id: Option<SymbolId> },
}

pub struct RuntimePipeline<S = Box<dyn Strategy + Send>> {
    strategy_engines: Vec<MarketEngine<S>>,
    order_engine: OrderThreadEngine,
    command_tx: Sender<StrategyCommand>,
    command_rx: Receiver<StrategyCommand>,
    exchange_event_tx: Sender<ExchangeEvent>,
    exchange_event_rx: Receiver<ExchangeEvent>,
}

impl<S: Strategy> RuntimePipeline<S> {
    pub fn new(
        config: RuntimePipelineConfig,
        strategies: Vec<S>,
        order_engine: OrderThreadEngine,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (exchange_event_tx, exchange_event_rx) = mpsc::channel();
        let strategy_engines = strategies
            .into_iter()
            .enumerate()
            .map(|(idx, strategy)| MarketEngine::new(SymbolId(idx as u16), strategy))
            .collect();
        let _ = config.command_capacity;
        let _ = config.exchange_event_capacity;
        Self {
            strategy_engines,
            order_engine,
            command_tx,
            command_rx,
            exchange_event_tx,
            exchange_event_rx,
        }
    }

    pub fn on_market_events(
        &mut self,
        events: Vec<MarketEvent>,
        sending_time: &str,
    ) -> Vec<RuntimePipelineStep> {
        let mut steps = Vec::new();
        for event in events {
            let Some(symbol_id) = market_event_symbol_id(&event) else {
                continue;
            };
            if let Some(engine) = self.strategy_engines.get_mut(symbol_id.0 as usize) {
                let commands = engine.on_market_event(event);
                steps.push(RuntimePipelineStep::MarketEventRouted { symbol_id });
                for command in commands {
                    if self.command_tx.send(command).is_ok() {
                        steps.push(RuntimePipelineStep::StrategyCommandRouted {
                            symbol_id: command.symbol_id(),
                        });
                    } else {
                        steps.push(RuntimePipelineStep::CommandDropped {
                            symbol_id: command.symbol_id(),
                        });
                    }
                }
            }
        }
        self.drain_commands(sending_time, &mut steps);
        steps
    }

    pub fn on_exchange_events(
        &mut self,
        events: Vec<ExchangeEvent>,
        sending_time: &str,
    ) -> Vec<RuntimePipelineStep> {
        let mut steps = Vec::new();
        for event in events {
            let symbol_id = event.symbol_id();
            if self.exchange_event_tx.send(event).is_ok() {
                steps.push(RuntimePipelineStep::ExchangeEventRouted { symbol_id });
            } else {
                steps.push(RuntimePipelineStep::ExchangeEventDropped { symbol_id });
            }
        }
        self.drain_exchange_events(&mut steps);
        self.drain_commands(sending_time, &mut steps);
        steps
    }

    pub fn command_sender(&self) -> Sender<StrategyCommand> {
        self.command_tx.clone()
    }

    pub fn exchange_event_sender(&self) -> Sender<ExchangeEvent> {
        self.exchange_event_tx.clone()
    }

    pub fn order_engine(&self) -> &OrderThreadEngine {
        &self.order_engine
    }

    fn drain_exchange_events(&mut self, steps: &mut Vec<RuntimePipelineStep>) {
        loop {
            match self.exchange_event_rx.try_recv() {
                Ok(event) => {
                    let symbol_id = event.symbol_id();
                    match symbol_id {
                        Some(symbol_id) => {
                            if let Some(engine) =
                                self.strategy_engines.get_mut(symbol_id.0 as usize)
                            {
                                for command in engine.on_exchange_event(event) {
                                    if self.command_tx.send(command).is_ok() {
                                        steps.push(RuntimePipelineStep::StrategyCommandRouted {
                                            symbol_id: command.symbol_id(),
                                        });
                                    } else {
                                        steps.push(RuntimePipelineStep::CommandDropped {
                                            symbol_id: command.symbol_id(),
                                        });
                                    }
                                }
                            }
                        }
                        None => {
                            for engine in &mut self.strategy_engines {
                                for command in engine.on_exchange_event(event.clone()) {
                                    if self.command_tx.send(command).is_ok() {
                                        steps.push(RuntimePipelineStep::StrategyCommandRouted {
                                            symbol_id: command.symbol_id(),
                                        });
                                    } else {
                                        steps.push(RuntimePipelineStep::CommandDropped {
                                            symbol_id: command.symbol_id(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn drain_commands(&mut self, sending_time: &str, steps: &mut Vec<RuntimePipelineStep>) {
        loop {
            match self.command_rx.try_recv() {
                Ok(command) => {
                    for action in self.order_engine.on_command(command, sending_time) {
                        steps.push(RuntimePipelineStep::OrderAction(action));
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }
}

fn market_event_symbol_id(event: &MarketEvent) -> Option<SymbolId> {
    match event {
        MarketEvent::L1 { symbol_id, .. } => Some(*symbol_id),
        MarketEvent::Trade(trade) => Some(trade.symbol_id),
    }
}

pub fn run_from_env() -> Result<(), RuntimeError> {
    let opts = RuntimeOptions::parse_args(env::args().skip(1))?;
    run(opts)
}

pub fn run(opts: RuntimeOptions) -> Result<(), RuntimeError> {
    let config_text = std::fs::read_to_string(&opts.config_path)?;
    let config = AppConfig::from_toml_str(&config_text)?;
    print_startup_summary(&opts, &config);

    if opts.dry_run {
        println!("[runtime] dry-run ok: config parsed; no network connections opened");
        return Ok(());
    }

    let credentials = credentials_from_env(&config)?;

    if opts.account {
        print_accounts_snapshot(&config, &credentials)?;
    }

    if opts.market_data && opts.order_entry {
        run_live_pipeline(&config, &credentials, opts.once)?;
    } else if opts.market_data {
        let shutdown = Arc::new(AtomicBool::new(false));
        run_fix_market_data(&config, &credentials, opts.once, shutdown, None)?;
    } else if opts.order_entry {
        let shutdown = Arc::new(AtomicBool::new(false));
        run_fix_order_entry(&config, &credentials, opts.once, shutdown, None, None)?;
    }

    Ok(())
}

fn usage() -> String {
    "Usage: cb-hft [--config PATH] [--dry-run] [--once] [--market-data-only|--order-only|--account-only|--with-order-entry|--with-account] [--no-market-data] [--no-order-entry] [--no-account]".to_string()
}

fn print_startup_summary(opts: &RuntimeOptions, config: &AppConfig) {
    println!("[runtime] config={}", opts.config_path);
    println!("[runtime] environment={}", config.coinbase.environment);
    println!("[runtime] products={}", product_ids(config).join(","));
    println!(
        "[runtime] fix_market_data={} fix_order_entry={} rest_account={} once={} dry_run={}",
        opts.market_data, opts.order_entry, opts.account, opts.once, opts.dry_run
    );
}

fn product_ids(config: &AppConfig) -> Vec<String> {
    config
        .products
        .iter()
        .map(|product| product.spec.coinbase_product.to_string())
        .collect()
}

fn credentials_from_env(config: &AppConfig) -> Result<CoinbaseCredentials, RuntimeError> {
    let mut missing = Vec::new();
    let api_key = env::var(&config.coinbase.api_key_env).map_err(|_| {
        missing.push(config.coinbase.api_key_env.clone());
    });
    let secret = env::var(&config.coinbase.api_secret_env).map_err(|_| {
        missing.push(config.coinbase.api_secret_env.clone());
    });
    let passphrase = env::var(&config.coinbase.passphrase_env).map_err(|_| {
        missing.push(config.coinbase.passphrase_env.clone());
    });

    if !missing.is_empty() {
        return Err(RuntimeError::MissingCredentials(missing));
    }

    Ok(CoinbaseCredentials::new(
        api_key.expect("checked missing api key"),
        passphrase.expect("checked missing passphrase"),
        secret.expect("checked missing secret"),
    ))
}

fn fix_md_host(environment: &str) -> &'static str {
    if environment.eq_ignore_ascii_case("prod") || environment.eq_ignore_ascii_case("production") {
        "fix-md.exchange.coinbase.com"
    } else {
        "fix-md.sandbox.exchange.coinbase.com"
    }
}

fn rest_base_url(environment: &str) -> &'static str {
    if environment.eq_ignore_ascii_case("prod") || environment.eq_ignore_ascii_case("production") {
        "https://api.exchange.coinbase.com"
    } else {
        "https://api-public.sandbox.exchange.coinbase.com"
    }
}

fn fix_ord_host(environment: &str) -> &'static str {
    if environment.eq_ignore_ascii_case("prod") || environment.eq_ignore_ascii_case("production") {
        "fix-ord.exchange.coinbase.com"
    } else {
        "fix-ord.sandbox.exchange.coinbase.com"
    }
}

fn timestamp_secs() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch");
    format!("{}.{:03}", now.as_secs(), now.subsec_millis())
}

fn fix_sending_time() -> Result<String, RuntimeError> {
    Ok(OffsetDateTime::now_utc().format(FIX_TIMESTAMP_FORMAT)?)
}

fn recv_ts_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

fn run_live_pipeline(
    config: &AppConfig,
    credentials: &CoinbaseCredentials,
    once: bool,
) -> Result<(), RuntimeError> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let (market_tx, market_rx) = mpsc::channel::<MarketEvent>();
    let (command_tx, command_rx) = mpsc::channel::<StrategyCommand>();
    let (exchange_event_tx, exchange_event_rx) = mpsc::channel::<ExchangeEvent>();

    let strategy_products: Vec<_> = config.products.iter().map(|product| product.spec).collect();
    let strategy_shutdown = Arc::clone(&shutdown);
    let strategy_handle = thread::spawn(move || {
        run_strategy_threads(
            strategy_products,
            market_rx,
            command_tx,
            exchange_event_rx,
            strategy_shutdown,
        )
    });

    let order_config = config.clone();
    let order_credentials = credentials.clone();
    let order_shutdown = Arc::clone(&shutdown);
    let order_handle = thread::spawn(move || {
        run_fix_order_entry(
            &order_config,
            &order_credentials,
            false,
            order_shutdown,
            Some(command_rx),
            Some(exchange_event_tx),
        )
    });

    let market_config = config.clone();
    let market_credentials = credentials.clone();
    let market_shutdown = Arc::clone(&shutdown);
    let market_handle = thread::spawn(move || {
        run_fix_market_data(
            &market_config,
            &market_credentials,
            once,
            market_shutdown,
            Some(market_tx),
        )
    });

    let market_result = join_runtime_thread("market", market_handle)?;
    shutdown.store(true, Ordering::SeqCst);

    join_runtime_thread("strategy", strategy_handle)?;
    join_runtime_thread("order", order_handle)??;
    market_result
}

fn run_strategy_threads(
    products: Vec<ProductSpec>,
    market_rx: Receiver<MarketEvent>,
    command_tx: Sender<StrategyCommand>,
    exchange_event_rx: Receiver<ExchangeEvent>,
    shutdown: Arc<AtomicBool>,
) {
    let mut engines: Vec<MarketEngine<NoopStrategy>> = products
        .iter()
        .map(|spec| MarketEngine::new(spec.symbol_id, NoopStrategy))
        .collect();

    while !shutdown.load(Ordering::SeqCst) {
        match market_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(event) => route_market_to_strategy(&mut engines, event, &command_tx),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        loop {
            match exchange_event_rx.try_recv() {
                Ok(event) => route_exchange_to_strategy(&mut engines, event, &command_tx),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }
}

fn route_market_to_strategy(
    engines: &mut [MarketEngine<NoopStrategy>],
    event: MarketEvent,
    command_tx: &Sender<StrategyCommand>,
) {
    let Some(symbol_id) = market_event_symbol_id(&event) else {
        return;
    };
    if let Some(engine) = engines.get_mut(symbol_id.0 as usize) {
        for command in engine.on_market_event(event) {
            let _ = command_tx.send(command);
        }
    }
}

fn route_exchange_to_strategy(
    engines: &mut [MarketEngine<NoopStrategy>],
    event: ExchangeEvent,
    command_tx: &Sender<StrategyCommand>,
) {
    match event.symbol_id() {
        Some(symbol_id) => {
            if let Some(engine) = engines.get_mut(symbol_id.0 as usize) {
                for command in engine.on_exchange_event(event) {
                    let _ = command_tx.send(command);
                }
            }
        }
        None => {
            for engine in engines {
                for command in engine.on_exchange_event(event.clone()) {
                    let _ = command_tx.send(command);
                }
            }
        }
    }
}

fn join_runtime_thread<T>(name: &str, handle: thread::JoinHandle<T>) -> Result<T, RuntimeError> {
    handle
        .join()
        .map_err(|_| RuntimeError::Protocol(format!("{name} thread panicked")))
}

fn run_fix_market_data(
    config: &AppConfig,
    credentials: &CoinbaseCredentials,
    once: bool,
    shutdown: Arc<AtomicBool>,
    market_event_tx: Option<Sender<MarketEvent>>,
) -> Result<(), RuntimeError> {
    let host = fix_md_host(&config.coinbase.environment);
    let addr = format!("{host}:6121");
    println!("[market.fix] connecting tcp+ssl://{addr} (Snapshot Enabled Gateway)");

    let tcp = TcpStream::connect(&addr)?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(Duration::from_secs(30)))?;
    let connector = TlsConnector::new()?;
    let mut stream = connector
        .connect(host, tcp)
        .map_err(|err| RuntimeError::Protocol(format!("tls handshake failed: {err}")))?;

    let encoder = FixEncoder::new("FIXT.1.1", &credentials.api_key, "Coinbase");
    let parser = FixParser::default();
    let mut sender_seq = 1u64;

    let logon_time = fix_sending_time()?;
    let logon = encoder.encode_coinbase_logon(sender_seq, &logon_time, 10, credentials, false)?;
    sender_seq += 1;
    stream.write_all(&logon)?;
    stream.flush()?;
    println!("[market.fix] sent Logon 35=A MsgSeqNum=1 TargetCompID=Coinbase");

    let mut subscribed = false;
    let mut read_buf = [0u8; 8192];
    let mut pending = Vec::<u8>::with_capacity(64 * 1024);
    let mut printed = 0usize;
    let mut l1_books = vec![L1Book::default(); config.products.len()];

    while !shutdown.load(Ordering::SeqCst) {
        let n = stream.read(&mut read_buf)?;
        if n == 0 {
            return Err(RuntimeError::Protocol(
                "FIX market data connection closed".to_string(),
            ));
        }
        pending.extend_from_slice(&read_buf[..n]);

        loop {
            let Some((frame, consumed)) = parser.next_frame(&pending)? else {
                break;
            };

            match frame.msg_type {
                MsgType::Logon => {
                    println!("[market.fix] received Logon 35=A; subscribing L1 depth + trades");
                    if !subscribed {
                        let symbols: Vec<&str> = config
                            .products
                            .iter()
                            .map(|product| product.spec.coinbase_product)
                            .collect();
                        let md_req = encoder.encode_market_data_request_with_depth(
                            sender_seq,
                            &fix_sending_time()?,
                            "cb-hft-l1-trades",
                            1,
                            &symbols,
                        );
                        sender_seq += 1;
                        stream.write_all(&md_req)?;
                        stream.flush()?;
                        subscribed = true;
                        println!(
                            "[market.fix] sent MarketDataRequest 35=V 263=1 264=1 symbols={}",
                            symbols.join(",")
                        );
                    }
                }
                MsgType::TestRequest => {
                    let heartbeat = encoder.encode_heartbeat(
                        sender_seq,
                        &fix_sending_time()?,
                        test_req_id(&parser, &frame),
                    );
                    sender_seq += 1;
                    stream.write_all(&heartbeat)?;
                    stream.flush()?;
                }
                MsgType::Heartbeat => {}
                MsgType::MarketDataSnapshotFullRefresh | MsgType::MarketDataIncrementalRefresh => {
                    let symbol = symbol_from_frame(&parser, &frame).ok_or_else(|| {
                        RuntimeError::Protocol("market data message missing Symbol(55)".to_string())
                    })?;
                    let (idx, spec) = product_by_symbol(config, symbol).ok_or_else(|| {
                        RuntimeError::Protocol(format!(
                            "market data message for unconfigured symbol {symbol}"
                        ))
                    })?;
                    let events = parse_market_data(&parser, &frame, spec, recv_ts_ns())?;
                    for event in events {
                        print_market_event(event, &mut l1_books[idx]);
                        if let Some(tx) = &market_event_tx {
                            if tx.send(event).is_err() {
                                return Err(RuntimeError::Protocol(
                                    "market pipeline receiver disconnected".to_string(),
                                ));
                            }
                        }
                        printed += 1;
                    }
                    if once && printed >= 5 {
                        shutdown.store(true, Ordering::SeqCst);
                    }
                }
                other => {
                    println!("[market.fix] received {other:?}");
                }
            }

            pending.drain(..consumed);
        }
    }

    Ok(())
}

fn run_fix_order_entry(
    config: &AppConfig,
    credentials: &CoinbaseCredentials,
    once: bool,
    shutdown: Arc<AtomicBool>,
    command_rx: Option<Receiver<StrategyCommand>>,
    exchange_event_tx: Option<Sender<ExchangeEvent>>,
) -> Result<(), RuntimeError> {
    let host = fix_ord_host(&config.coinbase.environment);
    let addr = format!("{host}:6121");
    println!("[order.fix] connecting tcp+ssl://{addr}");

    let tcp = TcpStream::connect(&addr)?;
    tcp.set_nodelay(true)?;
    tcp.set_read_timeout(Some(Duration::from_secs(30)))?;
    let connector = TlsConnector::new()?;
    let mut stream = connector
        .connect(host, tcp)
        .map_err(|err| RuntimeError::Protocol(format!("tls handshake failed: {err}")))?;

    let encoder = FixEncoder::new("FIXT.1.1", &credentials.api_key, "CBSE");
    let parser = FixParser::default();
    let mut sender_seq = 1u64;
    let manager = OrderManager::default();
    let products: Vec<_> = config.products.iter().map(|product| product.spec).collect();
    let mut order_engine = OrderThreadEngine::new(encoder.clone(), manager, products);

    let logon_time = fix_sending_time()?;
    let logon = encoder.encode_coinbase_logon(sender_seq, &logon_time, 10, credentials, true)?;
    sender_seq += 1;
    order_engine.set_next_seq_num(sender_seq);
    stream.write_all(&logon)?;
    stream.flush()?;
    println!("[order.fix] sent Logon 35=A MsgSeqNum=1 TargetCompID=CBSE CancelOnDisconnect=Y");

    let mut read_buf = [0u8; 8192];
    let mut pending = Vec::<u8>::with_capacity(64 * 1024);
    let mut printed = 0usize;

    while !shutdown.load(Ordering::SeqCst) {
        if let Some(rx) = &command_rx {
            loop {
                match rx.try_recv() {
                    Ok(command) => {
                        let actions = order_engine.on_command(command, &fix_sending_time()?);
                        for action in actions {
                            match action {
                                OrderThreadAction::SendFix(bytes) => {
                                    stream.write_all(&bytes)?;
                                    stream.flush()?;
                                    println!(
                                        "[order.fix] sent order command raw_len={}",
                                        bytes.len()
                                    );
                                }
                                OrderThreadAction::Reject(err) => {
                                    println!("[order.fix.reject] {err:?}");
                                }
                            }
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        return Err(RuntimeError::Protocol(
                            "strategy command sender disconnected".to_string(),
                        ));
                    }
                }
            }
        }

        let n = match stream.read(&mut read_buf) {
            Ok(n) => n,
            Err(err) if command_rx.is_some() && is_read_timeout(&err) => 0,
            Err(err) => return Err(RuntimeError::Io(err)),
        };
        if n == 0 && command_rx.is_none() {
            return Err(RuntimeError::Protocol(
                "FIX order-entry connection closed".to_string(),
            ));
        }
        if n == 0 {
            continue;
        }
        pending.extend_from_slice(&read_buf[..n]);

        loop {
            let Some((frame, consumed)) = parser.next_frame(&pending)? else {
                break;
            };

            match frame.msg_type {
                MsgType::Logon => {
                    println!("[order.fix] received Logon 35=A; order entry session ready");
                    if once {
                        shutdown.store(true, Ordering::SeqCst);
                    }
                }
                MsgType::TestRequest => {
                    let heartbeat = encoder.encode_heartbeat(
                        sender_seq,
                        &fix_sending_time()?,
                        test_req_id(&parser, &frame),
                    );
                    sender_seq += 1;
                    stream.write_all(&heartbeat)?;
                    stream.flush()?;
                }
                MsgType::Heartbeat => {}
                MsgType::ExecutionReport => {
                    let events = order_engine
                        .on_execution_report(&parser, &frame, recv_ts_ns())
                        .map_err(|err| {
                            RuntimeError::Protocol(format!("order report parse error: {err:?}"))
                        })?;
                    for event in events {
                        print_order_exchange_event(event.clone());
                        if let Some(tx) = &exchange_event_tx {
                            if tx.send(event).is_err() {
                                return Err(RuntimeError::Protocol(
                                    "strategy exchange-event receiver disconnected".to_string(),
                                ));
                            }
                        }
                        printed += 1;
                    }
                    if once && printed > 0 {
                        shutdown.store(true, Ordering::SeqCst);
                    }
                }
                MsgType::OrderCancelReject => {
                    println!(
                        "[order.fix] received OrderCancelReject 35=9 raw_len={}",
                        frame.raw.len()
                    );
                }
                other => {
                    println!("[order.fix] received {other:?}");
                }
            }

            pending.drain(..consumed);
        }
    }

    Ok(())
}

fn test_req_id<'a>(parser: &FixParser, frame: &'a crate::fix::FixFrame<'a>) -> Option<&'a str> {
    parser
        .fields(frame)
        .find(|field| field.tag == 112)
        .and_then(|field| std::str::from_utf8(field.value).ok())
}

fn symbol_from_frame<'a>(
    parser: &FixParser,
    frame: &'a crate::fix::FixFrame<'a>,
) -> Option<&'a str> {
    parser
        .fields(frame)
        .find(|field| field.tag == 55)
        .and_then(|field| std::str::from_utf8(field.value).ok())
}

fn is_read_timeout(err: &std::io::Error) -> bool {
    matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

fn product_by_symbol<'a>(config: &'a AppConfig, symbol: &str) -> Option<(usize, &'a ProductSpec)> {
    config
        .products
        .iter()
        .enumerate()
        .find(|(_, product)| product.spec.coinbase_product == symbol)
        .map(|(idx, product): (usize, &ProductConfig)| (idx, &product.spec))
}

fn print_market_event(event: MarketEvent, book: &mut L1Book) {
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
            println!(
                "[market.fix.l1] symbol_id={} bid_px={} bid_qty={} ask_px={} ask_qty={} seq={} recv_ts_ns={}",
                symbol_id.0, bid_px.0, bid_qty.0, ask_px.0, ask_qty.0, sequence, recv_ts_ns
            );
        }
        MarketEvent::Trade(trade) => println!(
            "[market.fix.trade] symbol_id={} trade_id={} price={} qty={} seq={} recv_ts_ns={}",
            trade.symbol_id.0,
            trade.trade_id,
            trade.price.0,
            trade.qty.0,
            trade.sequence,
            trade.recv_ts_ns
        ),
    }
}

fn print_order_exchange_event(event: ExchangeEvent) {
    match event {
        ExchangeEvent::Order(order) => println!(
            "[order.fix.event] symbol_id={} client_order_id={} exchange_order_id={} exec_id={} status={:?} filled_qty={} remaining_qty={} avg_px={} last_px={} last_qty={} seq={} recv_ts_ns={}",
            order.symbol_id.0,
            order.client_order_id,
            order.exchange_order_id,
            order.exec_id,
            order.status,
            order.filled_qty.0,
            order.remaining_qty.0,
            order.avg_fill_px.0,
            order.last_fill_px.0,
            order.last_fill_qty.0,
            order.sequence,
            order.recv_ts_ns
        ),
        ExchangeEvent::Fill(fill) => println!(
            "[order.fix.fill] symbol_id={} client_order_id={} exchange_order_id={} exec_id={} side={:?} price={} qty={} recv_ts_ns={}",
            fill.symbol_id.0,
            fill.client_order_id,
            fill.exchange_order_id,
            fill.exec_id,
            fill.side,
            fill.price.0,
            fill.qty.0,
            fill.recv_ts_ns
        ),
        other => println!("[order.fix.event] {other:?}"),
    }
}

fn print_accounts_snapshot(
    config: &AppConfig,
    credentials: &CoinbaseCredentials,
) -> Result<(), RuntimeError> {
    let timestamp = timestamp_secs();
    let path = "/accounts";
    let signature = CoinbaseAuth::sign_rest(credentials, &timestamp, "GET", path, "")?;
    let url = format!("{}{}", rest_base_url(&config.coinbase.environment), path);
    println!("[account] loading REST asset snapshot {url}");

    let response = ureq::get(&url)
        .set("CB-ACCESS-KEY", &credentials.api_key)
        .set("CB-ACCESS-SIGN", &signature)
        .set("CB-ACCESS-TIMESTAMP", &timestamp)
        .set("CB-ACCESS-PASSPHRASE", &credentials.passphrase)
        .set("User-Agent", "cb-hft/0.1")
        .call()
        .map_err(|err| RuntimeError::Http(err.to_string()))?;
    let body = response
        .into_string()
        .map_err(|err| RuntimeError::Http(err.to_string()))?;
    let snapshot =
        parse_rest_accounts_snapshot(body.as_bytes(), 10_000_000_000_000_000, recv_ts_ns())?;
    for balance in snapshot.balances() {
        println!(
            "[account.balance] asset={} total={} available={} hold={} recv_ts_ns={}",
            balance.asset_id.as_str(),
            balance.total.0,
            balance.available.0,
            balance.hold.0,
            balance.recv_ts_ns
        );
    }
    Ok(())
}
