//! Numeric-only operator lookup protocol. Exact trace ID; no SQL text or user data.
#![cfg(unix)]
use super::{trace::RequestTrace, wire};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

const QUERY_MAGIC: &[u8; 4] = b"OBQ1";
const REPLY_MAGIC: &[u8; 4] = b"OBR1";
pub const QUERY_SIZE: usize = 20;
pub const REPLY_MAX: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupResult {
    pub archived: u64,
    pub matches: u64,
    pub found: Option<(i64, RequestTrace)>,
}

pub fn query_frame(id: u128) -> [u8; QUERY_SIZE] {
    let mut frame = [0_u8; QUERY_SIZE];
    frame[..4].copy_from_slice(QUERY_MAGIC);
    frame[4..].copy_from_slice(&id.to_be_bytes());
    frame
}

pub fn parse_query(bytes: &[u8]) -> Option<u128> {
    (bytes.len() == QUERY_SIZE && bytes.starts_with(QUERY_MAGIC))
        .then(|| u128::from_be_bytes(bytes[4..].try_into().expect("checked length")))
}

pub fn encode_reply(result: &LookupResult) -> Option<Vec<u8>> {
    if result.matches > 1 || result.matches != u64::from(result.found.is_some())
        || result.matches > result.archived
    {
        return None;
    }
    let mut body = Vec::with_capacity(REPLY_MAX);
    body.extend_from_slice(REPLY_MAGIC);
    body.extend_from_slice(&result.archived.to_be_bytes());
    body.extend_from_slice(&result.matches.to_be_bytes());
    match &result.found {
        None => body.push(0),
        Some((at, trace)) => {
            let bytes = wire::encode(trace).ok()?;
            body.push(1);
            body.extend_from_slice(&at.to_be_bytes());
            body.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            body.extend_from_slice(&bytes);
        }
    }
    (body.len() <= REPLY_MAX).then_some(body)
}

pub fn decode_reply(bytes: &[u8]) -> Option<LookupResult> {
    if bytes.len() < 21 || bytes.len() > REPLY_MAX || !bytes.starts_with(REPLY_MAGIC) {
        return None;
    }
    let archived = u64::from_be_bytes(bytes[4..12].try_into().ok()?);
    let matches = u64::from_be_bytes(bytes[12..20].try_into().ok()?);
    let found = match bytes[20] {
        0 if bytes.len() == 21 && matches == 0 => None,
        1 if bytes.len() >= 31 && matches == 1 => {
            let at = i64::from_be_bytes(bytes[21..29].try_into().ok()?);
            let n = u16::from_be_bytes(bytes[29..31].try_into().ok()?) as usize;
            if n > wire::MAX_FRAME_BYTES || bytes.len() != 31 + n || at < 0 {
                return None;
            }
            Some((at, wire::decode(&bytes[31..]).ok()?))
        }
        _ => return None,
    };
    (matches <= archived).then_some(LookupResult {
        archived,
        matches,
        found,
    })
}

/// One bounded blocking IPC request. Always call through spawn_blocking from Axum.
pub fn request(socket: &Path, id: u128) -> Result<LookupResult, ()> {
    if !socket.is_absolute() {
        return Err(());
    }
    let mut stream = UnixStream::connect(socket).map_err(|_| ())?;
    stream
        .set_read_timeout(Some(Duration::from_millis(850)))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(850)))
        .map_err(|_| ())?;
    let bytes = query_frame(id);
    stream.write_all(&(bytes.len() as u32).to_be_bytes()).map_err(|_| ())?;
    stream.write_all(&bytes).map_err(|_| ())?;
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).map_err(|_| ())?;
    let n = u32::from_be_bytes(length) as usize;
    if !(21..=REPLY_MAX).contains(&n) {
        return Err(());
    }
    let mut bytes = vec![0_u8; n];
    stream.read_exact(&mut bytes).map_err(|_| ())?;
    let answer = decode_reply(&bytes).ok_or(())?;
    if answer.found.as_ref().is_some_and(|(_, t)| t.trace_id != id) {
        return Err(());
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RouteGroup, StatusGroup};

    fn trace() -> RequestTrace {
        RequestTrace {
            trace_id: 0xabcdef,
            route: RouteGroup::Reader,
            status: StatusGroup::ServerError,
            duration_us: 20_000,
            sqlx: None,
        }
    }

    #[test]
    fn exact_query_only_and_bounded_reply_roundtrip() {
        assert_eq!(parse_query(&query_frame(42)), Some(42));
        assert_eq!(parse_query(b"OBQ1"), None);
        assert_eq!(parse_query(b"OBQ1/secret?user_id=42"), None);
        let miss = LookupResult { archived: 151, matches: 0, found: None };
        assert_eq!(decode_reply(&encode_reply(&miss).unwrap()), Some(miss));
        let hit = LookupResult {
            archived: 151, matches: 1, found: Some((1_900_000_000, trace()))
        };
        let wire = encode_reply(&hit).unwrap();
        assert!(wire.len() <= REPLY_MAX);
        assert_eq!(decode_reply(&wire), Some(hit));
    }

    #[test]
    fn corrupted_or_mismatched_counts_fail_closed() {
        assert!(encode_reply(&LookupResult { archived: 0, matches: 1, found: None }).is_none());
        let mut wire = encode_reply(&LookupResult {
            archived: 1, matches: 1, found: Some((100, trace()))
        }).unwrap();
        wire[20] = 9;
        assert_eq!(decode_reply(&wire), None);
        let mut wire = encode_reply(&LookupResult {
            archived: 1, matches: 1, found: Some((100, trace()))
        }).unwrap();
        wire.push(0);
        assert_eq!(decode_reply(&wire), None);
    }
}
