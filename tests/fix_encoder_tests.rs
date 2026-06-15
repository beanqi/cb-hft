use cb_hft::fix::{FixEncoder, FixParser, MsgType};

const SOH: u8 = 0x01;

fn checksum_value(message: &[u8]) -> u32 {
    let checksum_start = message.windows(3).position(|w| w == b"10=").unwrap();
    std::str::from_utf8(&message[checksum_start + 3..checksum_start + 6])
        .unwrap()
        .parse()
        .unwrap()
}

fn calculated_checksum(message: &[u8]) -> u32 {
    let checksum_start = message.windows(3).position(|w| w == b"10=").unwrap();
    message[..checksum_start]
        .iter()
        .fold(0u32, |acc, b| acc + *b as u32)
        % 256
}

fn body_length_value(message: &[u8]) -> usize {
    let body_len_start = message.windows(2).position(|w| w == b"9=").unwrap() + 2;
    let body_len_end = message[body_len_start..]
        .iter()
        .position(|b| *b == SOH)
        .unwrap()
        + body_len_start;
    std::str::from_utf8(&message[body_len_start..body_len_end])
        .unwrap()
        .parse()
        .unwrap()
}

fn calculated_body_length(message: &[u8]) -> usize {
    let body_start = message.windows(3).position(|w| w == b"35=").unwrap();
    let checksum_start = message.windows(4).position(|w| w == b"\x0110=").unwrap() + 1;
    checksum_start - body_start
}

#[test]
fn encoder_calculates_body_length_and_checksum_for_every_message() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let messages = [
        encoder.encode_heartbeat(1, "20240613-12:00:00.000", None),
        encoder.encode_heartbeat(2, "20240613-12:00:01.000", Some("test-1")),
        encoder.encode_logon(3, "20240613-12:00:02.000", 30),
        encoder.encode_market_data_request(4, "20240613-12:00:03.000", "md-1", &["BTC-USD"]),
    ];

    for message in messages {
        assert_eq!(
            body_length_value(&message),
            calculated_body_length(&message)
        );
        assert_eq!(checksum_value(&message), calculated_checksum(&message));
        assert_eq!(message[message.len() - 1], SOH);
    }
}

#[test]
fn encoder_builds_heartbeat_and_optional_test_request_response_fields() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let message = encoder.encode_heartbeat(7, "20240613-12:34:56.789", Some("req-42"));
    let parser = FixParser::default();

    let (frame, consumed) = parser.next_frame(&message).unwrap().unwrap();

    assert_eq!(consumed, message.len());
    assert_eq!(frame.msg_type, MsgType::Heartbeat);
    assert!(message.windows(4).any(|w| w == b"35=0"));
    assert!(message.windows(5).any(|w| w == b"34=7\x01"));
    assert!(message.windows(10).any(|w| w == b"49=SENDER\x01"));
    assert!(message.windows(10).any(|w| w == b"56=TARGET\x01"));
    assert!(message.windows(10).any(|w| w == b"112=req-42"));
}

#[test]
fn encoder_builds_logon_basics() {
    let encoder = FixEncoder::new("FIX.4.2", "CLIENT", "COINBASE");
    let message = encoder.encode_logon(1, "20240613-12:00:00.000", 30);
    let parser = FixParser::default();

    let (frame, _) = parser.next_frame(&message).unwrap().unwrap();

    assert_eq!(frame.msg_type, MsgType::Logon);
    assert!(message.windows(4).any(|w| w == b"35=A"));
    assert!(message.windows(4).any(|w| w == b"98=0"));
    assert!(message.windows(6).any(|w| w == b"108=30"));
}

#[test]
fn encoder_builds_market_data_request_basics() {
    let encoder = FixEncoder::new("FIX.4.2", "CLIENT", "COINBASE");
    let message = encoder.encode_market_data_request(
        9,
        "20240613-12:00:01.000",
        "book-BTC-USD",
        &["BTC-USD", "ETH-USD"],
    );
    let parser = FixParser::default();

    let (frame, _) = parser.next_frame(&message).unwrap().unwrap();

    assert_eq!(frame.msg_type, MsgType::MarketDataRequest);
    assert!(message.windows(4).any(|w| w == b"35=V"));
    assert!(message.windows(16).any(|w| w == b"262=book-BTC-USD"));
    assert!(message.windows(5).any(|w| w == b"263=1"));
    assert!(message.windows(5).any(|w| w == b"264=1"));
    assert!(!message.windows(5).any(|w| w == b"267="));
    assert!(!message.windows(5).any(|w| w == b"269="));
    assert!(message.windows(5).any(|w| w == b"146=2"));
    assert!(message.windows(10).any(|w| w == b"55=BTC-USD"));
    assert!(message.windows(10).any(|w| w == b"55=ETH-USD"));
}
