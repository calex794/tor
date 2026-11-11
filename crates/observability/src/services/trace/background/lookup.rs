//! Bounded exact-parent lookup, separate from the unchanged HTTP root protocol.
use super::{BackgroundTrace, FRAME_SIZE};
use std::{
    collections::HashSet,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

pub const LIMIT: usize = 32;
pub const REPLY_MAX: usize = 13 + LIMIT * (8 + FRAME_SIZE);
const QUERY_MAGIC: &[u8; 4] = b"OBGQ";
const REPLY_MAGIC: &[u8; 4] = b"OBGR";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackgroundPage {
    /// Retained matching attempts, not all attempts ever executed.
    pub matches: u64,
    pub items: Vec<(i64, BackgroundTrace)>,
}

pub fn query_frame(parent: u128) -> [u8; 20] {
    let mut bytes = [0_u8; 20];
    bytes[..4].copy_from_slice(QUERY_MAGIC);
    bytes[4..].copy_from_slice(&parent.to_be_bytes());
    bytes
}

pub fn parse_query(bytes: &[u8]) -> Option<u128> {
    if bytes.len() != 20 || !bytes.starts_with(QUERY_MAGIC) {
        return None;
    }
    Some(u128::from_be_bytes(bytes[4..].try_into().ok()?))
}

pub fn encode_reply(page: &BackgroundPage) -> Option<Vec<u8>> {
    if page.matches > 1_000_000 || page.items.len() != page.matches.min(LIMIT as u64) as usize {
        return None;
    }
    let mut ids = HashSet::new();
    let mut bytes = Vec::with_capacity(REPLY_MAX);
    bytes.extend_from_slice(REPLY_MAGIC);
    bytes.extend_from_slice(&page.matches.to_be_bytes());
    bytes.push(page.items.len() as u8);
    for (at, trace) in &page.items {
        if *at < 0 || !ids.insert(trace.trace_id) {
            return None;
        }
        bytes.extend_from_slice(&at.to_be_bytes());
        bytes.extend_from_slice(&trace.encode()?);
    }
    Some(bytes)
}

pub fn decode_reply(bytes: &[u8], parent: u128) -> Option<BackgroundPage> {
    if bytes.len() < 13 || bytes.len() > REPLY_MAX || !bytes.starts_with(REPLY_MAGIC) {
        return None;
    }
    let matches = u64::from_be_bytes(bytes[4..12].try_into().ok()?);
    let count = usize::from(bytes[12]);
    if count > LIMIT || bytes.len() != 13 + count * (8 + FRAME_SIZE) {
        return None;
    }
    let mut items = Vec::with_capacity(count);
    for chunk in bytes[13..].chunks_exact(8 + FRAME_SIZE) {
        let at = i64::from_be_bytes(chunk[..8].try_into().ok()?);
        let trace = BackgroundTrace::decode(&chunk[8..])?;
        if trace.parent_trace_id != parent {
            return None;
        }
        items.push((at, trace));
    }
    let page = BackgroundPage { matches, items };
    encode_reply(&page)?;
    Some(page)
}

/// Blocking local IPC only; Axum must call this through spawn_blocking.
pub fn request(socket: &Path, parent: u128) -> Result<BackgroundPage, std::io::Error> {
    let invalid = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    if !socket.is_absolute() {
        return Err(invalid());
    }
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_millis(850)))?;
    stream.set_write_timeout(Some(Duration::from_millis(850)))?;
    let query = query_frame(parent);
    stream.write_all(&(query.len() as u32).to_be_bytes())?;
    stream.write_all(&query)?;
    let mut size = [0_u8; 4];
    stream.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if !(13..=REPLY_MAX).contains(&size) {
        return Err(invalid());
    }
    let mut reply = vec![0_u8; size];
    stream.read_exact(&mut reply)?;
    decode_reply(&reply, parent).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::trace::background::Outcome;

    fn page() -> BackgroundPage {
        BackgroundPage {
            matches: 1,
            items: vec![(
                100,
                BackgroundTrace {
                    parent_trace_id: 16,
                    trace_id: 32,
                    attempt: 1,
                    outcome: Outcome::Retried,
                    phase: 1,
                    duration_us: 10,
                    phase_us: [10, 0, 0, 0],
                },
            )],
        }
    }

    #[test]
    fn exact_parent_roundtrip_and_empty_retained_page() {
        assert_eq!(parse_query(&query_frame(16)), Some(16));
        assert_eq!(parse_query(b"OBGQ"), None);
        let value = page();
        let frame = encode_reply(&value).unwrap();
        assert_eq!(decode_reply(&frame, 16), Some(value));
        assert!(decode_reply(&frame, 17).is_none());
        let empty = BackgroundPage::default();
        assert_eq!(decode_reply(&encode_reply(&empty).unwrap(), 16), Some(empty));
    }

    #[test]
    fn incomplete_duplicate_and_oversized_replies_fail_closed() {
        let frame = encode_reply(&page()).unwrap();
        for n in 0..frame.len() {
            assert!(decode_reply(&frame[..n], 16).is_none());
        }
        let mut value = page();
        value.matches = 2;
        assert!(encode_reply(&value).is_none());
        value.items.push(value.items[0].clone());
        assert!(encode_reply(&value).is_none());
        assert!(decode_reply(&vec![0; REPLY_MAX + 1], 16).is_none());
    }
}
