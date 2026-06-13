use cb_hft::fix::{FixEncoder, FixParser, MsgType};
use cb_hft::types::Side;

fn field(message: &[u8], tag: u32) -> Option<&[u8]> {
    let prefix = format!("{tag}=");
    message
        .split(|b| *b == 1)
        .find_map(|raw| raw.strip_prefix(prefix.as_bytes()))
}

#[test]
fn encoder_builds_limit_new_order_single_with_post_only_exec_inst() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");

    let message = encoder.encode_limit_new_order_single(
        7,
        "20260613-12:00:00.000",
        "cbhft-1",
        "BTC-USD",
        Side::Buy,
        "65000.12",
        "0.01",
        true,
    );

    let parser = FixParser::default();
    let (frame, consumed) = parser.next_frame(&message).unwrap().unwrap();

    assert_eq!(consumed, message.len());
    assert_eq!(frame.msg_type, MsgType::NewOrderSingle);
    assert_eq!(field(&message, 11), Some(&b"cbhft-1"[..]));
    assert_eq!(field(&message, 55), Some(&b"BTC-USD"[..]));
    assert_eq!(field(&message, 54), Some(&b"1"[..]));
    assert_eq!(field(&message, 38), Some(&b"0.01"[..]));
    assert_eq!(field(&message, 40), Some(&b"2"[..]));
    assert_eq!(field(&message, 44), Some(&b"65000.12"[..]));
    assert_eq!(field(&message, 18), Some(&b"6"[..]));
}

#[test]
fn encoder_builds_order_cancel_request() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");

    let message = encoder.encode_order_cancel_request(
        8,
        "20260613-12:00:01.000",
        "cancel-1",
        "cbhft-1",
        "BTC-USD",
        Side::Sell,
    );

    let parser = FixParser::default();
    let (frame, consumed) = parser.next_frame(&message).unwrap().unwrap();

    assert_eq!(consumed, message.len());
    assert_eq!(frame.msg_type, MsgType::OrderCancelRequest);
    assert_eq!(field(&message, 11), Some(&b"cancel-1"[..]));
    assert_eq!(field(&message, 41), Some(&b"cbhft-1"[..]));
    assert_eq!(field(&message, 55), Some(&b"BTC-USD"[..]));
    assert_eq!(field(&message, 54), Some(&b"2"[..]));
}
