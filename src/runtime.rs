use std::env;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tungstenite::{Message, connect};

use crate::config::AppConfig;
use crate::fix::coinbase::auth::{CoinbaseAuth, CoinbaseCredentials};

#[derive(Clone, Debug)]
pub struct RuntimeOptions {
    pub config_path: String,
    pub dry_run: bool,
    pub market_data: bool,
    pub account: bool,
    pub once: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            config_path: "config/sandbox.toml.example".to_string(),
            dry_run: false,
            market_data: true,
            account: true,
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
                "--market-data-only" => {
                    opts.market_data = true;
                    opts.account = false;
                }
                "--account-only" => {
                    opts.market_data = false;
                    opts.account = true;
                }
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
    Config(crate::config::ConfigError),
    MissingCredentials(Vec<String>),
    WebSocket(tungstenite::Error),
    Http(String),
    Json(serde_json::Error),
    Auth(crate::fix::coinbase::auth::CoinbaseAuthError),
    ThreadPanic(&'static str),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(msg) => write!(f, "{msg}"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Config(err) => write!(f, "config error: {err:?}"),
            Self::MissingCredentials(names) => {
                write!(
                    f,
                    "missing credential environment variables: {}",
                    names.join(", ")
                )
            }
            Self::WebSocket(err) => write!(f, "websocket error: {err}"),
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Json(err) => write!(f, "json error: {err}"),
            Self::Auth(err) => write!(f, "auth error: {err}"),
            Self::ThreadPanic(name) => write!(f, "{name} thread panicked"),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<std::io::Error> for RuntimeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<crate::config::ConfigError> for RuntimeError {
    fn from(value: crate::config::ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<tungstenite::Error> for RuntimeError {
    fn from(value: tungstenite::Error) -> Self {
        Self::WebSocket(value)
    }
}

impl From<serde_json::Error> for RuntimeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<crate::fix::coinbase::auth::CoinbaseAuthError> for RuntimeError {
    fn from(value: crate::fix::coinbase::auth::CoinbaseAuthError) -> Self {
        Self::Auth(value)
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

    let shutdown = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    let credentials = match credentials_from_env(&config) {
        Ok(credentials) => Some(credentials),
        Err(err) if opts.account && !opts.market_data => return Err(err),
        Err(err) => {
            eprintln!("[account] credentials not loaded: {err}");
            eprintln!(
                "[account] public market data still works; set the configured API key/secret/passphrase env vars to enable REST assets, user order/fill feed, and authenticated level2 depth"
            );
            None
        }
    };

    if opts.market_data {
        let products = product_ids(&config);
        let env_name = config.coinbase.environment.clone();
        let once = opts.once;
        let shutdown = shutdown.clone();
        let market_credentials = credentials.clone();
        handles.push((
            "market-data",
            thread::Builder::new()
                .name("cb-ws-market-data".to_string())
                .spawn(move || {
                    run_market_ws(&env_name, &products, market_credentials, once, shutdown)
                })?,
        ));
    }

    if opts.account {
        if let Some(credentials) = credentials {
            print_accounts_snapshot(&config, &credentials)?;
            let products = product_ids(&config);
            let env_name = config.coinbase.environment.clone();
            let once = opts.once;
            let shutdown = shutdown.clone();
            handles.push((
                "user-feed",
                thread::Builder::new()
                    .name("cb-ws-user-feed".to_string())
                    .spawn(move || {
                        run_authenticated_user_ws(&env_name, &products, credentials, once, shutdown)
                    })?,
            ));
        }
    }

    for (name, handle) in handles {
        let result = handle.join().map_err(|_| RuntimeError::ThreadPanic(name))?;
        if let Err(err) = result {
            shutdown.store(true, Ordering::SeqCst);
            return Err(err);
        }
    }

    Ok(())
}

fn usage() -> String {
    "Usage: cb-hft [--config PATH] [--dry-run] [--once] [--market-data-only|--account-only] [--no-market-data] [--no-account]".to_string()
}

fn print_startup_summary(opts: &RuntimeOptions, config: &AppConfig) {
    println!("[runtime] config={}", opts.config_path);
    println!("[runtime] environment={}", config.coinbase.environment);
    println!("[runtime] products={}", product_ids(config).join(","));
    println!(
        "[runtime] market_data={} account={} once={} dry_run={}",
        opts.market_data, opts.account, opts.once, opts.dry_run
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
        ()
    });
    let secret = env::var(&config.coinbase.api_secret_env).map_err(|_| {
        missing.push(config.coinbase.api_secret_env.clone());
        ()
    });
    let passphrase = env::var(&config.coinbase.passphrase_env).map_err(|_| {
        missing.push(config.coinbase.passphrase_env.clone());
        ()
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

fn ws_url(environment: &str) -> &'static str {
    if environment.eq_ignore_ascii_case("prod") || environment.eq_ignore_ascii_case("production") {
        "wss://ws-feed.exchange.coinbase.com"
    } else {
        "wss://ws-feed-public.sandbox.exchange.coinbase.com"
    }
}

fn rest_base_url(environment: &str) -> &'static str {
    if environment.eq_ignore_ascii_case("prod") || environment.eq_ignore_ascii_case("production") {
        "https://api.exchange.coinbase.com"
    } else {
        "https://api-public.sandbox.exchange.coinbase.com"
    }
}

fn timestamp_secs() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch");
    format!("{}.{:03}", now.as_secs(), now.subsec_millis())
}

fn run_market_ws(
    environment: &str,
    products: &[String],
    credentials: Option<CoinbaseCredentials>,
    once: bool,
    shutdown: Arc<AtomicBool>,
) -> Result<(), RuntimeError> {
    let url = ws_url(environment);
    println!("[market] connecting {url}");
    let (mut socket, _) = connect(url)?;
    let product_values: Vec<Value> = products.iter().cloned().map(Value::String).collect();
    let subscribe = if let Some(credentials) = credentials.as_ref() {
        let timestamp = timestamp_secs();
        let product_refs: Vec<&str> = products.iter().map(String::as_str).collect();
        let signed = CoinbaseAuth::websocket_subscribe_json(
            credentials,
            &timestamp,
            &["ticker", "matches", "level2"],
            &product_refs,
        )?;
        serde_json::from_str::<Value>(&signed)?
    } else {
        serde_json::json!({
            "type": "subscribe",
            "product_ids": product_values,
            "channels": ["ticker", "matches"]
        })
    };
    socket.send(Message::Text(subscribe.to_string()))?;
    if credentials.is_some() {
        println!("[market] subscribed authenticated ticker,matches,level2");
    } else {
        println!(
            "[market] subscribed public ticker,matches; level2 depth requires Coinbase authentication"
        );
    }

    let mut printed = 0usize;
    while !shutdown.load(Ordering::SeqCst) {
        let message = socket.read()?;
        let Message::Text(text) = message else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            print_market_message(&value);
        } else {
            println!("[market.raw] {text}");
        }
        printed += 1;
        if once && printed >= 5 {
            shutdown.store(true, Ordering::SeqCst);
            break;
        }
    }
    Ok(())
}

fn run_authenticated_user_ws(
    environment: &str,
    products: &[String],
    credentials: CoinbaseCredentials,
    once: bool,
    shutdown: Arc<AtomicBool>,
) -> Result<(), RuntimeError> {
    let url = ws_url(environment);
    println!("[user] connecting {url}");
    let (mut socket, _) = connect(url)?;
    let timestamp = timestamp_secs();
    let product_refs: Vec<&str> = products.iter().map(String::as_str).collect();
    let subscribe = CoinbaseAuth::websocket_subscribe_json(
        &credentials,
        &timestamp,
        &["user", "full"],
        &product_refs,
    )?;
    socket.send(Message::Text(subscribe))?;
    println!("[user] subscribed user/full channel");

    let mut printed = 0usize;
    while !shutdown.load(Ordering::SeqCst) {
        let message = socket.read()?;
        let Message::Text(text) = message else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            print_user_message(&value);
        } else {
            println!("[user.raw] {text}");
        }
        printed += 1;
        if once && printed >= 5 {
            shutdown.store(true, Ordering::SeqCst);
            break;
        }
    }
    Ok(())
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
    let value: Value = serde_json::from_str(&body)?;
    print_account_snapshot_value(&value);
    Ok(())
}

fn print_market_message(value: &Value) {
    let typ = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match typ {
        "ticker" => println!(
            "[market.l1] product={} bid={} bid_size={} ask={} ask_size={} price={} time={} seq={}",
            str_field(value, "product_id"),
            str_field(value, "best_bid"),
            str_field(value, "best_bid_size"),
            str_field(value, "best_ask"),
            str_field(value, "best_ask_size"),
            str_field(value, "price"),
            str_field(value, "time"),
            value
                .get("sequence")
                .map(Value::to_string)
                .unwrap_or_default(),
        ),
        "match" | "last_match" => println!(
            "[market.trade] product={} side={} price={} size={} trade_id={} time={} seq={}",
            str_field(value, "product_id"),
            str_field(value, "side"),
            str_field(value, "price"),
            str_field(value, "size"),
            value
                .get("trade_id")
                .map(Value::to_string)
                .unwrap_or_default(),
            str_field(value, "time"),
            value
                .get("sequence")
                .map(Value::to_string)
                .unwrap_or_default(),
        ),
        "snapshot" => println!(
            "[market.depth.snapshot] product={} bids={} asks={}",
            str_field(value, "product_id"),
            value
                .get("bids")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            value
                .get("asks")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
        ),
        "l2update" => println!(
            "[market.depth.update] product={} changes={} time={}",
            str_field(value, "product_id"),
            value
                .get("changes")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            str_field(value, "time"),
        ),
        "subscriptions" => println!("[market.subscriptions] {value}"),
        "error" => eprintln!("[market.error] {value}"),
        _ => println!("[market.{typ}] {value}"),
    }
}

fn print_user_message(value: &Value) {
    let typ = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match typ {
        "open" | "done" | "received" | "change" => println!(
            "[user.order] type={} product={} order_id={} client_oid={} side={} price={} size={} remaining={} reason={} seq={}",
            typ,
            str_field(value, "product_id"),
            str_field(value, "order_id"),
            str_field(value, "client_oid"),
            str_field(value, "side"),
            str_field(value, "price"),
            str_field(value, "size"),
            str_field(value, "remaining_size"),
            str_field(value, "reason"),
            value
                .get("sequence")
                .map(Value::to_string)
                .unwrap_or_default(),
        ),
        "match" => println!(
            "[user.fill] product={} trade_id={} maker_order_id={} taker_order_id={} side={} price={} size={} fee={} seq={}",
            str_field(value, "product_id"),
            value
                .get("trade_id")
                .map(Value::to_string)
                .unwrap_or_default(),
            str_field(value, "maker_order_id"),
            str_field(value, "taker_order_id"),
            str_field(value, "side"),
            str_field(value, "price"),
            str_field(value, "size"),
            str_field(value, "taker_fee_rate"),
            value
                .get("sequence")
                .map(Value::to_string)
                .unwrap_or_default(),
        ),
        "subscriptions" => println!("[user.subscriptions] {value}"),
        "error" => eprintln!("[user.error] {value}"),
        _ => println!("[user.{typ}] {value}"),
    }
}

fn print_account_snapshot_value(value: &Value) {
    let Some(accounts) = value.as_array() else {
        println!("[account.assets.raw] {value}");
        return;
    };
    println!("[account.assets] count={}", accounts.len());
    for account in accounts {
        println!(
            "[account.asset] currency={} balance={} available={} hold={} id={}",
            str_field(account, "currency"),
            str_field(account, "balance"),
            str_field(account, "available"),
            str_field(account, "hold"),
            str_field(account, "id"),
        );
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn sleep_forever() {
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
