use cb_hft::account::{UserFeedEvent, parse_rest_accounts_snapshot, parse_user_feed_event_json};
use cb_hft::event::ExchangeEvent;
use cb_hft::fix::coinbase::auth::{CoinbaseAuth, CoinbaseCredentials};
use cb_hft::fix::{FixEncoder, FixParser};
use cb_hft::order::OrderStatus;
use cb_hft::types::{AssetId, Qty};

fn field(message: &[u8], tag: u32) -> Option<&[u8]> {
    let prefix = format!("{tag}=");
    message
        .split(|b| *b == 1)
        .find_map(|raw| raw.strip_prefix(prefix.as_bytes()))
}

fn credentials() -> CoinbaseCredentials {
    CoinbaseCredentials::new(
        "api-key",
        "passphrase",
        "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
    )
}

#[test]
fn coinbase_fix_signature_matches_hmac_sha256_fixture() {
    let signature =
        CoinbaseAuth::sign_fix_logon(&credentials(), "20230822-20:43:30.000", "A", 1, "CBSE")
            .unwrap();

    assert_eq!(signature, "5odPCzOuZnxvgJJ1sXh2pIiBe5v4i181+1jcppc7IAA=");
}

#[test]
fn coinbase_rest_and_ws_signature_matches_documented_prehash_shape() {
    let signature = CoinbaseAuth::sign_rest(
        &credentials(),
        "1234567890.123",
        "GET",
        "/users/self/verify",
        "",
    )
    .unwrap();

    assert_eq!(signature, "hwVYUr0mCQtkc5vFOP0gvreM/lx9Vh+pft2Eet++0PU=");
}

#[test]
fn fix_encoder_builds_coinbase_authenticated_logon_fields() {
    let encoder = FixEncoder::new("FIXT.1.1", "api-key", "CBSE");
    let message = encoder
        .encode_coinbase_logon(1, "20230822-20:43:30.000", 30, &credentials(), true)
        .unwrap();

    assert_eq!(field(&message, 35), Some(&b"A"[..]));
    assert_eq!(field(&message, 34), Some(&b"1"[..]));
    assert_eq!(field(&message, 553), Some(&b"api-key"[..]));
    assert_eq!(field(&message, 554), Some(&b"passphrase"[..]));
    assert_eq!(
        field(&message, 96),
        Some(&b"5odPCzOuZnxvgJJ1sXh2pIiBe5v4i181+1jcppc7IAA="[..])
    );
    assert_eq!(field(&message, 95), Some(&b"44"[..]));
    assert_eq!(field(&message, 1137), Some(&b"9"[..]));
    assert_eq!(field(&message, 8013), Some(&b"Y"[..]));

    let parser = FixParser::default();
    assert!(parser.next_frame(&message).unwrap().is_some());
}

#[test]
fn websocket_authenticated_subscribe_message_uses_verify_signature() {
    let json = CoinbaseAuth::websocket_subscribe_json(
        &credentials(),
        "1234567890.123",
        &["user"],
        &["BTC-USD"],
    )
    .unwrap();

    assert!(json.contains("\"type\":\"subscribe\""));
    assert!(json.contains("\"channels\":[\"user\"]"));
    assert!(json.contains("\"product_ids\":[\"BTC-USD\"]"));
    assert!(json.contains("\"signature\":\"hwVYUr0mCQtkc5vFOP0gvreM/lx9Vh+pft2Eet++0PU=\""));
    assert!(json.contains("\"key\":\"api-key\""));
    assert!(json.contains("\"passphrase\":\"passphrase\""));
}

#[test]
fn websocket_user_open_done_match_messages_decode_to_exchange_events() {
    let open = r#"{
        "type":"open",
        "product_id":"BTC-USD",
        "order_id":"ex-1",
        "client_oid":"cid-1",
        "price":"200.2",
        "remaining_size":"1.00",
        "side":"sell"
    }"#;
    let done = r#"{
        "type":"done",
        "product_id":"BTC-USD",
        "order_id":"ex-1",
        "client_oid":"cid-1",
        "reason":"filled",
        "remaining_size":"0",
        "side":"sell"
    }"#;
    let fill = r#"{
        "type":"match",
        "product_id":"BTC-USD",
        "trade_id":10,
        "maker_order_id":"ex-maker",
        "taker_order_id":"ex-taker",
        "price":"400.23",
        "size":"5.23512",
        "side":"sell",
        "client_oid":"cid-fill"
    }"#;

    let open_event = parse_user_feed_event_json(open.as_bytes(), 1, 100, 100_000_000)
        .unwrap()
        .into_exchange_event();
    let done_event = parse_user_feed_event_json(done.as_bytes(), 2, 100, 100_000_000)
        .unwrap()
        .into_exchange_event();
    let fill_event = parse_user_feed_event_json(fill.as_bytes(), 3, 100, 100_000_000)
        .unwrap()
        .into_exchange_event();

    assert!(
        matches!(open_event, ExchangeEvent::Order(ref event) if event.status == OrderStatus::Open)
    );
    assert!(
        matches!(done_event, ExchangeEvent::Order(ref event) if event.status == OrderStatus::Filled)
    );
    assert!(matches!(fill_event, ExchangeEvent::Fill(_)));
}

#[test]
fn rest_accounts_response_decodes_to_balance_snapshot() {
    let json = r#"[
      {
        "id": "7fd0abc0-e5ad-4cbb-8d54-f2b3f43364da",
        "currency": "USD",
        "balance": "10.5000000000000000",
        "hold": "1.2500000000000000",
        "available": "9.2500000000000000",
        "profile_id": "8058d771-2d88-4f0f-ab6e-299c153d4308",
        "trading_enabled": true
      }
    ]"#;

    let snapshot =
        parse_rest_accounts_snapshot(json.as_bytes(), 10_000_000_000_000_000, 9).unwrap();
    let balance = snapshot.balances().first().unwrap();

    assert_eq!(balance.asset_id, AssetId::from_static("USD"));
    assert_eq!(balance.total, Qty(105_000_000_000_000_000));
    assert_eq!(balance.available, Qty(92_500_000_000_000_000));
    assert_eq!(balance.hold, Qty(12_500_000_000_000_000));
}

#[test]
fn websocket_balance_fixture_decodes_to_balance_event() {
    let json = r#"{
        "type":"balance",
        "currency":"USD",
        "balance":"10.50",
        "available":"9.25",
        "hold":"1.25"
    }"#;

    let event = parse_user_feed_event_json(json.as_bytes(), 7, 100, 100).unwrap();

    assert!(matches!(event, UserFeedEvent::Balance(_)));
}
