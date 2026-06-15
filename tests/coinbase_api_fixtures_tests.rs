use cb_hft::account::parse_rest_accounts_snapshot;
use cb_hft::fix::coinbase::auth::{CoinbaseAuth, CoinbaseCredentials};
use cb_hft::fix::{FixEncoder, FixParser};
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

    assert_eq!(signature, "p9lbo4RxkkKEpHoQoTUxzB1sB+svVqdUZtZ0+QkNKNQ=");
}

#[test]
fn coinbase_rest_signature_matches_documented_prehash_shape() {
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
        Some(&b"p9lbo4RxkkKEpHoQoTUxzB1sB+svVqdUZtZ0+QkNKNQ="[..])
    );
    assert_eq!(field(&message, 95), Some(&b"44"[..]));
    assert_eq!(field(&message, 1137), Some(&b"9"[..]));
    assert_eq!(field(&message, 8013), Some(&b"Y"[..]));

    let parser = FixParser::default();
    assert!(parser.next_frame(&message).unwrap().is_some());
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
