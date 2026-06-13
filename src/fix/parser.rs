use core::fmt;

use super::{SOH, checksum, find_byte, parse_u32};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixError {
    MissingBeginString,
    MissingBodyLength,
    InvalidBodyLength,
    MissingChecksum,
    InvalidChecksum,
    InvalidTag,
    MissingMsgType,
}

impl fmt::Display for FixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for FixError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MsgType {
    Heartbeat,
    TestRequest,
    Logon,
    NewOrderSingle,
    OrderCancelRequest,
    MarketDataRequest,
    MarketDataSnapshotFullRefresh,
    MarketDataIncrementalRefresh,
    ExecutionReport,
    OrderCancelReject,
    Other(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixFrame<'a> {
    pub raw: &'a [u8],
    pub body: &'a [u8],
    pub msg_type: MsgType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixTag<'a> {
    pub tag: u32,
    pub value: &'a [u8],
}

#[derive(Default)]
pub struct FixParser;

impl FixParser {
    pub fn next_frame<'a>(&self, buf: &'a [u8]) -> Result<Option<(FixFrame<'a>, usize)>, FixError> {
        if buf.is_empty() {
            return Ok(None);
        }
        if !buf.starts_with(b"8=") {
            return Err(FixError::MissingBeginString);
        }

        let begin_end = match find_byte(buf, SOH, 0) {
            Some(pos) => pos + 1,
            None => return Ok(None),
        };

        let body_len_tag = b"9=";
        if !buf
            .get(begin_end..)
            .is_some_and(|tail| tail.starts_with(body_len_tag))
        {
            return Err(FixError::MissingBodyLength);
        }
        let body_len_value_start = begin_end + body_len_tag.len();
        let body_len_end = match find_byte(buf, SOH, body_len_value_start) {
            Some(pos) => pos,
            None => return Ok(None),
        };
        let body_len = parse_u32(&buf[body_len_value_start..body_len_end])
            .map_err(|_| FixError::InvalidBodyLength)? as usize;
        let body_start = body_len_end + 1;
        let body_end = body_start
            .checked_add(body_len)
            .ok_or(FixError::InvalidBodyLength)?;

        if body_end > buf.len() {
            if contains_checksum_tag(&buf[body_start..]) {
                return Err(FixError::InvalidBodyLength);
            }
            return Ok(None);
        }
        if body_end == buf.len() {
            return Ok(None);
        }

        if !buf
            .get(body_end..)
            .is_some_and(|tail| tail.starts_with(b"10="))
        {
            return Err(FixError::InvalidBodyLength);
        }
        let checksum_value_start = body_end + 3;
        let checksum_end = checksum_value_start + 3;
        if checksum_end >= buf.len() {
            return Ok(None);
        }
        if buf[checksum_end] != SOH {
            return Err(FixError::MissingChecksum);
        }
        let expected_checksum = parse_u32(&buf[checksum_value_start..checksum_end])
            .map_err(|_| FixError::InvalidChecksum)?;
        let actual_checksum = checksum(&buf[..body_end]);
        if expected_checksum != actual_checksum {
            return Err(FixError::InvalidChecksum);
        }

        let body = &buf[body_start..body_end];
        let msg_type = msg_type_from_body(body)?;
        let raw_end = checksum_end + 1;
        Ok(Some((
            FixFrame {
                raw: &buf[..raw_end],
                body,
                msg_type,
            },
            raw_end,
        )))
    }

    pub fn fields<'a>(&self, frame: &'a FixFrame<'a>) -> FixFieldIter<'a> {
        FixFieldIter {
            body: frame.body,
            offset: 0,
        }
    }
}

pub struct FixFieldIter<'a> {
    body: &'a [u8],
    offset: usize,
}

impl<'a> Iterator for FixFieldIter<'a> {
    type Item = FixTag<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset < self.body.len() && self.body[self.offset] == SOH {
            self.offset += 1;
        }
        if self.offset >= self.body.len() {
            return None;
        }
        let start = self.offset;
        let end = find_byte(self.body, SOH, start).unwrap_or(self.body.len());
        self.offset = end.saturating_add(1);
        let eq = self.body[start..end].iter().position(|b| *b == b'=')? + start;
        let tag = parse_u32(&self.body[start..eq]).ok()?;
        let value = &self.body[eq + 1..end];
        Some(FixTag { tag, value })
    }
}

fn msg_type_from_body(body: &[u8]) -> Result<MsgType, FixError> {
    let mut offset = 0;
    while offset < body.len() {
        let end = find_byte(body, SOH, offset).unwrap_or(body.len());
        if body.get(offset..offset + 3) == Some(b"35=") {
            let value = &body[offset + 3..end];
            return match value {
                b"0" => Ok(MsgType::Heartbeat),
                b"1" => Ok(MsgType::TestRequest),
                b"A" => Ok(MsgType::Logon),
                b"D" => Ok(MsgType::NewOrderSingle),
                b"F" => Ok(MsgType::OrderCancelRequest),
                b"V" => Ok(MsgType::MarketDataRequest),
                b"W" => Ok(MsgType::MarketDataSnapshotFullRefresh),
                b"X" => Ok(MsgType::MarketDataIncrementalRefresh),
                b"8" => Ok(MsgType::ExecutionReport),
                b"9" => Ok(MsgType::OrderCancelReject),
                [single] => Ok(MsgType::Other(*single)),
                _ => Err(FixError::MissingMsgType),
            };
        }
        offset = end.saturating_add(1);
    }
    Err(FixError::MissingMsgType)
}

fn contains_checksum_tag(input: &[u8]) -> bool {
    input.windows(4).any(|window| window == b"\x0110=")
}
