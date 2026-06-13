use cb_hft::fix::{FixParser, FixTag, MsgType};

const SOH: u8 = 0x01;

fn fix_message(body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"8=FIX.4.2\x01");
    msg.extend_from_slice(format!("9={}\x01", body.len()).as_bytes());
    msg.extend_from_slice(body);
    let checksum = msg.iter().fold(0u32, |acc, b| acc + *b as u32) % 256;
    msg.extend_from_slice(format!("10={checksum:03}\x01").as_bytes());
    msg
}

#[test]
fn parser_returns_none_for_partial_fix_frame() {
    let parser = FixParser::default();
    let partial = b"8=FIX.4.2\x019=5\x0135=0\x01";

    assert!(parser.next_frame(partial).unwrap().is_none());
}

#[test]
fn parser_extracts_single_frame_and_msg_type() {
    let parser = FixParser::default();
    let msg = fix_message(b"35=0\x01");

    let (frame, consumed) = parser.next_frame(&msg).unwrap().unwrap();

    assert_eq!(consumed, msg.len());
    assert_eq!(frame.msg_type, MsgType::Heartbeat);
    assert_eq!(frame.body, b"35=0\x01");
    assert_eq!(frame.raw, msg.as_slice());
}

#[test]
fn parser_consumes_only_first_frame_when_buffer_has_multiple_messages() {
    let parser = FixParser::default();
    let first = fix_message(b"35=0\x01");
    let second = fix_message(b"35=1\x01112=req-1\x01");
    let mut combined = first.clone();
    combined.extend_from_slice(&second);

    let (frame, consumed) = parser.next_frame(&combined).unwrap().unwrap();

    assert_eq!(frame.msg_type, MsgType::Heartbeat);
    assert_eq!(consumed, first.len());
    let (next_frame, next_consumed) = parser.next_frame(&combined[consumed..]).unwrap().unwrap();
    assert_eq!(next_frame.msg_type, MsgType::TestRequest);
    assert_eq!(next_consumed, second.len());
}

#[test]
fn field_iterator_yields_numeric_tags_and_raw_values_without_soh() {
    let parser = FixParser::default();
    let msg = fix_message(b"35=1\x01112=req-1\x01");
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let fields: Vec<_> = parser.fields(&frame).collect();

    assert_eq!(
        fields[0],
        FixTag {
            tag: 35,
            value: b"1"
        }
    );
    assert_eq!(
        fields[1],
        FixTag {
            tag: 112,
            value: b"req-1"
        }
    );
}

#[test]
fn parser_rejects_bad_checksum() {
    let parser = FixParser::default();
    let mut msg = fix_message(b"35=0\x01");
    let checksum_start = msg.windows(3).position(|w| w == b"10=").unwrap() + 3;
    msg[checksum_start] = b'9';

    assert!(parser.next_frame(&msg).is_err());
}

#[test]
fn parser_rejects_bad_body_length() {
    let parser = FixParser::default();
    let msg = b"8=FIX.4.2\x019=999\x0135=0\x0110=000\x01";

    assert!(parser.next_frame(msg).is_err());
}

#[test]
fn soh_constant_matches_fix_delimiter() {
    assert_eq!(SOH, 1);
}
