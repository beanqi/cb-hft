use cb_hft::event::SessionEvent;
use cb_hft::fix::{FixEncoder, FixParser, FixSession, SessionAction, SessionState};

fn fix_message(body: &[u8]) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(b"8=FIX.4.2\x01");
    msg.extend_from_slice(format!("9={}\x01", body.len()).as_bytes());
    msg.extend_from_slice(body);
    let checksum = msg.iter().fold(0u32, |acc, b| acc + *b as u32) % 256;
    msg.extend_from_slice(format!("10={checksum:03}\x01").as_bytes());
    msg
}

fn field(message: &[u8], tag: u32) -> Option<&[u8]> {
    let prefix = format!("{tag}=");
    message
        .split(|b| *b == 1)
        .find_map(|raw| raw.strip_prefix(prefix.as_bytes()))
}

#[test]
fn fix_session_builds_logon_and_advances_outbound_sequence() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let mut session = FixSession::new(encoder, 30);

    let logon = session.build_logon("20260613-12:00:00.000");

    assert_eq!(session.state(), SessionState::LogonSent);
    assert_eq!(session.next_sender_seq_num(), 2);
    assert_eq!(field(&logon, 35), Some(&b"A"[..]));
    assert_eq!(field(&logon, 34), Some(&b"1"[..]));
    assert_eq!(field(&logon, 108), Some(&b"30"[..]));
}

#[test]
fn fix_session_marks_connected_when_logon_is_received_in_sequence() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let parser = FixParser::default();
    let mut session = FixSession::new(encoder, 30);
    let msg = fix_message(
        b"35=A\x0134=1\x0149=TARGET\x0156=SENDER\x0152=20260613-12:00:00.000\x0198=0\x01108=30\x01",
    );
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let actions = session.on_inbound(&parser, &frame, "20260613-12:00:00.001");

    assert_eq!(session.state(), SessionState::Connected);
    assert_eq!(session.next_target_seq_num(), 2);
    assert_eq!(actions, vec![SessionAction::Emit(SessionEvent::Connected)]);
}

#[test]
fn fix_session_responds_to_test_request_with_heartbeat() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let parser = FixParser::default();
    let mut session = FixSession::new(encoder, 30);
    let msg = fix_message(
        b"35=1\x0134=1\x0149=TARGET\x0156=SENDER\x0152=20260613-12:00:01.000\x01112=req-1\x01",
    );
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let actions = session.on_inbound(&parser, &frame, "20260613-12:00:01.001");

    assert_eq!(session.next_target_seq_num(), 2);
    assert_eq!(session.next_sender_seq_num(), 2);
    assert_eq!(actions.len(), 1);
    match &actions[0] {
        SessionAction::Send(bytes) => {
            assert_eq!(field(bytes, 35), Some(&b"0"[..]));
            assert_eq!(field(bytes, 34), Some(&b"1"[..]));
            assert_eq!(field(bytes, 112), Some(&b"req-1"[..]));
        }
        other => panic!("unexpected action: {other:?}"),
    }
}

#[test]
fn fix_session_detects_sequence_gap() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let parser = FixParser::default();
    let mut session = FixSession::new(encoder, 30);
    let msg =
        fix_message(b"35=0\x0134=3\x0149=TARGET\x0156=SENDER\x0152=20260613-12:00:02.000\x01");
    let (frame, _) = parser.next_frame(&msg).unwrap().unwrap();

    let actions = session.on_inbound(&parser, &frame, "20260613-12:00:02.001");

    assert_eq!(session.next_target_seq_num(), 1);
    assert_eq!(
        actions,
        vec![SessionAction::Emit(SessionEvent::SequenceGap {
            expected: 1,
            received: 3,
        })]
    );
}

#[test]
fn fix_session_can_build_regular_heartbeat() {
    let encoder = FixEncoder::new("FIX.4.2", "SENDER", "TARGET");
    let mut session = FixSession::new(encoder, 30);

    let heartbeat = session.build_heartbeat("20260613-12:00:03.000");

    assert_eq!(session.next_sender_seq_num(), 2);
    assert_eq!(field(&heartbeat, 35), Some(&b"0"[..]));
    assert_eq!(field(&heartbeat, 34), Some(&b"1"[..]));
}
