pub mod coinbase;
mod encoder;
mod parser;
mod session;

pub use encoder::FixEncoder;
pub use parser::{FixError, FixFieldIter, FixFrame, FixParser, FixTag, MsgType};
pub use session::{FixSession, SessionAction, SessionState};

pub(crate) const SOH: u8 = 0x01;

pub(crate) fn find_byte(buf: &[u8], byte: u8, from: usize) -> Option<usize> {
    buf.get(from..)?
        .iter()
        .position(|b| *b == byte)
        .map(|pos| pos + from)
}

pub(crate) fn parse_u32(input: &[u8]) -> Result<u32, ()> {
    if input.is_empty() {
        return Err(());
    }
    let mut value = 0u32;
    for &b in input {
        if !b.is_ascii_digit() {
            return Err(());
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add((b - b'0') as u32))
            .ok_or(())?;
    }
    Ok(value)
}

pub(crate) fn checksum(input: &[u8]) -> u32 {
    input.iter().fold(0u32, |acc, b| acc + *b as u32) % 256
}
