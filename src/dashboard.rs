use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, DashboardSecrets, persist_dashboard_update};
use crate::runtime::RuntimeError;

#[derive(Default)]
struct DashboardState {
    child: Option<Child>,
    last_error: Option<String>,
}

#[derive(Deserialize)]
struct DashboardUpdate {
    api_key: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
    trend_window_ms: Option<u64>,
    min_window_notional: Option<i64>,
    strong_score_x100: Option<i64>,
    quote_qty_by_symbol: Option<std::collections::HashMap<String, String>>,
}

#[derive(Serialize)]
struct PublicState {
    running: bool,
    pid: Option<u32>,
    products: Vec<ProductState>,
    api_key_configured: bool,
    api_secret_configured: bool,
    passphrase_configured: bool,
    trend_window_ms: u64,
    min_window_notional: i128,
    strong_score_x100: i64,
    quote_tick_offset_ticks: i64,
    requote_ticks: i64,
    last_error: Option<String>,
}

#[derive(Serialize)]
struct ProductState {
    symbol: String,
    quote_qty: i64,
    price_tick: i64,
    qty_step: i64,
}

pub fn serve(config_path: String) -> Result<(), RuntimeError> {
    let bind =
        std::env::var("CB_HFT_DASHBOARD_BIND").unwrap_or_else(|_| "127.0.0.1:8088".to_string());
    let listener = TcpListener::bind(&bind)?;
    println!("[dashboard] http://{bind}");
    let state = Arc::new(Mutex::new(DashboardState::default()));
    for stream in listener.incoming() {
        let stream = stream?;
        let state = Arc::clone(&state);
        let config_path = config_path.clone();
        thread::spawn(move || {
            if let Err(err) = handle(stream, state, config_path) {
                eprintln!("[dashboard] request error: {err}");
            }
        });
    }
    Ok(())
}

fn handle(
    mut stream: TcpStream,
    state: Arc<Mutex<DashboardState>>,
    config_path: String,
) -> Result<(), RuntimeError> {
    let mut buf = [0u8; 16384];
    let n = stream.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut parts = req.lines().next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or("/");
    let body = req.split("\r\n\r\n").nth(1).unwrap_or_default();
    match (method, path.split('?').next().unwrap_or(path)) {
        ("GET", "/") => respond_html(&mut stream, INDEX_HTML),
        ("GET", "/api/state") => respond_json(&mut stream, &public_state(&state, &config_path)?),
        ("GET", "/api/feed") => respond_text(&mut stream, 200, &read_feed(&config_path)),
        ("POST", "/api/start") => {
            start_runtime(&state, &config_path)?;
            respond_json(&mut stream, &public_state(&state, &config_path)?)
        }
        ("POST", "/api/stop") => {
            stop_runtime(&state);
            respond_json(&mut stream, &public_state(&state, &config_path)?)
        }
        ("POST", "/api/config") => {
            save_config(&config_path, body)?;
            respond_json(&mut stream, &public_state(&state, &config_path)?)
        }
        _ => respond_text(&mut stream, 404, "not found"),
    }
}

fn public_state(
    state: &Arc<Mutex<DashboardState>>,
    config_path: &str,
) -> Result<PublicState, RuntimeError> {
    let config_text = std::fs::read_to_string(config_path)?;
    let config = AppConfig::from_toml_str(&config_text)?;
    let mut guard = state.lock().expect("dashboard state poisoned");
    if let Some(child) = guard.child.as_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            guard.last_error = (!status.success()).then(|| format!("runtime exited: {status}"));
            guard.child = None;
        }
    }
    Ok(PublicState {
        running: guard.child.is_some(),
        pid: guard.child.as_ref().map(|child| child.id()),
        products: config
            .products
            .iter()
            .map(|product| ProductState {
                symbol: product.spec.coinbase_product.to_string(),
                quote_qty: product.maker_quote_qty.0,
                price_tick: product.spec.price_tick.0,
                qty_step: product.spec.qty_step.0,
            })
            .collect(),
        api_key_configured: config.coinbase.api_key.is_some()
            || std::env::var(&config.coinbase.api_key_env).is_ok(),
        api_secret_configured: config.coinbase.api_secret.is_some()
            || std::env::var(&config.coinbase.api_secret_env).is_ok(),
        passphrase_configured: config.coinbase.passphrase.is_some()
            || std::env::var(&config.coinbase.passphrase_env).is_ok(),
        trend_window_ms: config.strategy.trend.window_ns / 1_000_000,
        min_window_notional: config.strategy.trend.min_window_notional,
        strong_score_x100: config.strategy.trend.strong_score_x100,
        quote_tick_offset_ticks: config.strategy.quote_tick_offset_ticks,
        requote_ticks: config.strategy.requote_ticks,
        last_error: guard.last_error.clone(),
    })
}

fn start_runtime(
    state: &Arc<Mutex<DashboardState>>,
    config_path: &str,
) -> Result<(), RuntimeError> {
    let mut guard = state.lock().expect("dashboard state poisoned");
    if guard.child.is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let feed_file = feed_path(config_path);
    let _ = std::fs::write(&feed_file, "");
    let child = Command::new(exe)
        .arg("--config")
        .arg(config_path)
        .arg("--with-order-entry")
        .env("CB_HFT_FEED_FILE", &feed_file)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    guard.last_error = None;
    guard.child = Some(child);
    Ok(())
}

fn stop_runtime(state: &Arc<Mutex<DashboardState>>) {
    let mut guard = state.lock().expect("dashboard state poisoned");
    if let Some(mut child) = guard.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn save_config(config_path: &str, body: &str) -> Result<(), RuntimeError> {
    let update: DashboardUpdate = serde_json::from_str(body)
        .map_err(|err| RuntimeError::Protocol(format!("json decode error: {err}")))?;
    persist_dashboard_update(
        config_path,
        DashboardSecrets {
            api_key: non_empty(update.api_key),
            api_secret: non_empty(update.api_secret),
            passphrase: non_empty(update.passphrase),
            trend_window_ms: update.trend_window_ms,
            min_window_notional: update.min_window_notional,
            strong_score_x100: update.strong_score_x100,
            quote_qty_by_symbol: update
                .quote_qty_by_symbol
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(symbol, qty)| non_empty(Some(qty)).map(|qty| (symbol, qty)))
                .collect(),
        },
    )
    .map_err(RuntimeError::Config)
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn feed_path(config_path: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let name = config_path.replace(['/', '\\', ':'], "_");
    path.push(format!("cb-hft-feed-{name}.jsonl"));
    path
}

fn read_feed(config_path: &str) -> String {
    std::fs::read_to_string(feed_path(config_path)).unwrap_or_default()
}

fn respond_json<T: Serialize>(stream: &mut TcpStream, value: &T) -> Result<(), RuntimeError> {
    let body = serde_json::to_string(value)
        .map_err(|err| RuntimeError::Protocol(format!("json encode error: {err}")))?;
    respond(stream, 200, "application/json", &body)
}

fn respond_html(stream: &mut TcpStream, body: &str) -> Result<(), RuntimeError> {
    respond(stream, 200, "text/html; charset=utf-8", body)
}

fn respond_text(stream: &mut TcpStream, status: u16, body: &str) -> Result<(), RuntimeError> {
    respond(stream, status, "text/plain; charset=utf-8", body)
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), RuntimeError> {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    Ok(())
}

const INDEX_HTML: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><title>cb-hft</title><style>
body{font-family:system-ui;background:#0b1020;color:#d7e0ff;margin:24px}.card{background:#151b2f;border:1px solid #2d365d;border-radius:12px;padding:16px;margin:12px 0}button{background:#4d7cff;color:white;border:0;border-radius:8px;padding:10px 16px;margin-right:8px}button.stop{background:#e24a68}input{background:#090d1a;color:#d7e0ff;border:1px solid #2d365d;border-radius:8px;padding:8px;margin:4px;min-width:120px}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(280px,1fr));gap:12px}pre{white-space:pre-wrap;max-height:360px;overflow:auto}</style></head>
<body><h1>cb-hft Coinbase FIX 做市控制台</h1>
<div class="card"><button onclick="post('/api/start')">启动策略</button><button class="stop" onclick="post('/api/stop')">停止策略</button><button onclick="load()">刷新</button></div>
<div class="card"><h3>API Key / 策略参数</h3><input id="k" placeholder="API Key"><input id="s" placeholder="Secret"><input id="p" placeholder="Passphrase"><input id="tw" placeholder="趋势窗口ms"><input id="mn" placeholder="最小成交额"><input id="ss" placeholder="强趋势score_x100"><div id="qtys"></div><button onclick="save()">保存到配置文件</button></div>
<div class="grid"><div class="card"><h3>状态</h3><pre id="state">loading...</pre></div><div class="card"><h3>FIX 事件流</h3><pre id="feed">等待启动后行情/订单事件...</pre></div></div>
<script>
let products=[]; async function load(){let r=await fetch('/api/state');let j=await r.json();products=j.products||[];state.textContent=JSON.stringify(j,null,2);qtys.innerHTML=products.map(p=>`<input class="qty" data-sym="${p.symbol}" placeholder="${p.symbol} quote_qty, scaled=${p.quote_qty}">`).join('');let f=await fetch('/api/feed');feed.textContent=await f.text()||'暂无事件'}
async function post(p){await fetch(p,{method:'POST'});load()}
async function save(){let q={};document.querySelectorAll('.qty').forEach(i=>{if(i.value)q[i.dataset.sym]=i.value});await fetch('/api/config',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({api_key:k.value,api_secret:s.value,passphrase:p.value,trend_window_ms:num(tw.value),min_window_notional:num(mn.value),strong_score_x100:num(ss.value),quote_qty_by_symbol:q})});k.value=s.value=p.value=tw.value=mn.value=ss.value='';load()}
function num(v){return v?Number(v):null} load(); setInterval(load,2000)
</script></body></html>"#;
