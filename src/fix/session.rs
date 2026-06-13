use crate::event::SessionEvent;

use super::{FixEncoder, FixFrame, FixParser, MsgType};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Disconnected,
    LogonSent,
    Connected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionAction {
    Send(Vec<u8>),
    Emit(SessionEvent),
}

pub struct FixSession {
    encoder: FixEncoder,
    heartbeat_interval_secs: u64,
    state: SessionState,
    next_sender_seq_num: u64,
    next_target_seq_num: u64,
}

impl FixSession {
    pub fn new(encoder: FixEncoder, heartbeat_interval_secs: u64) -> Self {
        Self {
            encoder,
            heartbeat_interval_secs,
            state: SessionState::Disconnected,
            next_sender_seq_num: 1,
            next_target_seq_num: 1,
        }
    }

    pub fn build_logon(&mut self, sending_time: &str) -> Vec<u8> {
        let seq = self.take_sender_seq_num();
        self.state = SessionState::LogonSent;
        self.encoder
            .encode_logon(seq, sending_time, self.heartbeat_interval_secs)
    }

    pub fn build_heartbeat(&mut self, sending_time: &str) -> Vec<u8> {
        let seq = self.take_sender_seq_num();
        self.encoder.encode_heartbeat(seq, sending_time, None)
    }

    pub fn on_inbound(
        &mut self,
        parser: &FixParser,
        frame: &FixFrame<'_>,
        sending_time: &str,
    ) -> Vec<SessionAction> {
        let Some(received_seq) = inbound_seq_num(parser, frame) else {
            return Vec::new();
        };
        if received_seq != self.next_target_seq_num {
            return vec![SessionAction::Emit(SessionEvent::SequenceGap {
                expected: self.next_target_seq_num,
                received: received_seq,
            })];
        }
        self.next_target_seq_num += 1;

        match frame.msg_type {
            MsgType::Logon => {
                self.state = SessionState::Connected;
                vec![SessionAction::Emit(SessionEvent::Connected)]
            }
            MsgType::TestRequest => {
                let test_req_id = test_req_id(parser, frame)
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
                let seq = self.take_sender_seq_num();
                let heartbeat =
                    self.encoder
                        .encode_heartbeat(seq, sending_time, test_req_id.as_deref());
                vec![SessionAction::Send(heartbeat)]
            }
            _ => Vec::new(),
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    pub fn next_sender_seq_num(&self) -> u64 {
        self.next_sender_seq_num
    }

    pub fn next_target_seq_num(&self) -> u64 {
        self.next_target_seq_num
    }

    fn take_sender_seq_num(&mut self) -> u64 {
        let seq = self.next_sender_seq_num;
        self.next_sender_seq_num += 1;
        seq
    }
}

fn inbound_seq_num(parser: &FixParser, frame: &FixFrame<'_>) -> Option<u64> {
    parser
        .fields(frame)
        .find(|field| field.tag == 34)
        .and_then(|field| parse_u64(field.value))
}

fn test_req_id<'a>(parser: &FixParser, frame: &'a FixFrame<'a>) -> Option<&'a [u8]> {
    parser
        .fields(frame)
        .find(|field| field.tag == 112)
        .map(|field| field.value)
}

fn parse_u64(input: &[u8]) -> Option<u64> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for &b in input {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(value)
}
