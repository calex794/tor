//! Strict, numeric-only trace-store protocol v1: no arbitrary text fields.
//! One length-prefixed frame holds one complete already-sampled HTTP trace.
use crate::{
    domain::{RouteGroup, StatusGroup},
    services::trace::{RequestTrace, SqlEvidence, SqlSpan, MAX_SQL_SPANS},
};

pub const MAX_FRAME_BYTES: usize = 192;
const MAGIC: &[u8; 4] = b"OBS1";
const MAX_DURATION_US: u64 = 86_400_000_000; // 24 h: reject corrupted clocks.
const MAX_QUERIES: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    InvalidFrame,
    InvalidValue,
}

fn read<const N: usize>(data: &[u8], cursor: &mut usize) -> Result<[u8; N], WireError> {
    let end = cursor.checked_add(N).ok_or(WireError::InvalidFrame)?;
    let part = data.get(*cursor..end).ok_or(WireError::InvalidFrame)?;
    *cursor = end;
    part.try_into().map_err(|_| WireError::InvalidFrame)
}

fn group(code: u8) -> Result<RouteGroup, WireError> {
    match code {
        0 => Ok(RouteGroup::Home),
        1 => Ok(RouteGroup::Catalog),
        2 => Ok(RouteGroup::Books),
        3 => Ok(RouteGroup::Reader),
        4 => Ok(RouteGroup::Other),
        _ => Err(WireError::InvalidValue),
    }
}

fn status(code: u8) -> Result<StatusGroup, WireError> {
    match code {
        0 => Ok(StatusGroup::Success),
        1 => Ok(StatusGroup::Redirect),
        2 => Ok(StatusGroup::ClientError),
        3 => Ok(StatusGroup::ServerError),
        4 => Ok(StatusGroup::Other),
        _ => Err(WireError::InvalidValue),
    }
}

pub fn encode(trace: &RequestTrace) -> Result<Vec<u8>, WireError> {
    if trace.duration_us > MAX_DURATION_US {
        return Err(WireError::InvalidValue);
    }
    let mut out = Vec::with_capacity(MAX_FRAME_BYTES);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&trace.trace_id.to_be_bytes());
    out.push(trace.route as u8);
    out.push(trace.status as u8);
    out.extend_from_slice(&trace.duration_us.to_be_bytes());
    match &trace.sqlx {
        None => {
            out.push(0);
            out.extend_from_slice(&[0; 25]);
        }
        Some(sql) => {
            if sql.spans.len() > MAX_SQL_SPANS
                || sql.observed_queries > MAX_QUERIES
                || sql.observed_elapsed_us > MAX_DURATION_US
                || sql.uncaptured_spans > MAX_QUERIES
                || sql.spans.len() as u64 > sql.observed_queries
                || sql.uncaptured_spans > sql.observed_queries
                || sql.spans.len() as u64 + sql.uncaptured_spans > sql.observed_queries
            {
                return Err(WireError::InvalidValue);
            }
            out.push(1);
            out.extend_from_slice(&sql.observed_queries.to_be_bytes());
            out.extend_from_slice(&sql.observed_elapsed_us.to_be_bytes());
            out.extend_from_slice(&sql.uncaptured_spans.to_be_bytes());
            out.push(sql.spans.len() as u8);
            for span in &sql.spans {
                if span.completed_at_us > trace.duration_us || span.elapsed_us > MAX_DURATION_US {
                    return Err(WireError::InvalidValue);
                }
                out.extend_from_slice(&span.completed_at_us.to_be_bytes());
                out.extend_from_slice(&span.elapsed_us.to_be_bytes());
            }
        }
    }
    if out.len() > MAX_FRAME_BYTES {
        return Err(WireError::InvalidFrame);
    }
    Ok(out)
}

pub fn decode(data: &[u8]) -> Result<RequestTrace, WireError> {
    if data.len() < 55 || data.len() > MAX_FRAME_BYTES {
        return Err(WireError::InvalidFrame);
    }
    let mut i = 0;
    if &read::<4>(data, &mut i)? != MAGIC {
        return Err(WireError::InvalidFrame);
    }
    let trace_id = u128::from_be_bytes(read(data, &mut i)?);
    let route = group(read::<1>(data, &mut i)?[0])?;
    let status = status(read::<1>(data, &mut i)?[0])?;
    let duration_us = u64::from_be_bytes(read(data, &mut i)?);
    let present = read::<1>(data, &mut i)?[0];
    let queries = u64::from_be_bytes(read(data, &mut i)?);
    let elapsed = u64::from_be_bytes(read(data, &mut i)?);
    let uncaptured = u64::from_be_bytes(read(data, &mut i)?);
    let count = usize::from(read::<1>(data, &mut i)?[0]);
    if duration_us > MAX_DURATION_US || count > MAX_SQL_SPANS {
        return Err(WireError::InvalidValue);
    }
    let sqlx = match present {
        0 if queries == 0 && elapsed == 0 && uncaptured == 0 && count == 0 => None,
        1 if queries <= MAX_QUERIES
            && elapsed <= MAX_DURATION_US
            && uncaptured <= queries
            && count as u64 + uncaptured <= queries =>
        {
            let mut spans = Vec::with_capacity(count);
            for _ in 0..count {
                let completed_at_us = u64::from_be_bytes(read(data, &mut i)?);
                let elapsed_us = u64::from_be_bytes(read(data, &mut i)?);
                if completed_at_us > duration_us || elapsed_us > MAX_DURATION_US {
                    return Err(WireError::InvalidValue);
                }
                spans.push(SqlSpan {
                    completed_at_us,
                    elapsed_us,
                });
            }
            Some(SqlEvidence {
                observed_queries: queries,
                observed_elapsed_us: elapsed,
                spans,
                uncaptured_spans: uncaptured,
            })
        }
        _ => return Err(WireError::InvalidValue),
    };
    if i != data.len() {
        return Err(WireError::InvalidFrame);
    }
    Ok(RequestTrace {
        trace_id,
        route,
        status,
        duration_us,
        sqlx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RequestTrace {
        RequestTrace {
            trace_id: 123,
            route: RouteGroup::Reader,
            status: StatusGroup::ServerError,
            duration_us: 35_000,
            sqlx: Some(SqlEvidence {
                observed_queries: 2,
                observed_elapsed_us: 10_000,
                spans: vec![
                    SqlSpan {
                        completed_at_us: 10_000,
                        elapsed_us: 4_000,
                    },
                    SqlSpan {
                        completed_at_us: 20_000,
                        elapsed_us: 6_000,
                    },
                ],
                uncaptured_spans: 0,
            }),
        }
    }

    #[test]
    fn complete_trace_roundtrip_and_unknown_sql_state() {
        let trace = sample();
        assert_eq!(decode(&encode(&trace).unwrap()), Ok(trace.clone()));
        let mut unknown = trace;
        unknown.sqlx = None;
        assert_eq!(decode(&encode(&unknown).unwrap()), Ok(unknown));
    }

    #[test]
    fn corrupt_and_oversized_frames_are_rejected() {
        let bytes = encode(&sample()).unwrap();
        for truncated in 0..bytes.len() {
            assert!(decode(&bytes[..truncated]).is_err());
        }
        let mut wrong = bytes.clone();
        wrong[0] = b'Z';
        assert_eq!(decode(&wrong), Err(WireError::InvalidFrame));
        let mut wrong = bytes.clone();
        wrong[20] = 255; // route group
        assert_eq!(decode(&wrong), Err(WireError::InvalidValue));
        let mut wrong = bytes;
        wrong.extend_from_slice(b"raw path /private?token=secret");
        assert!(decode(&wrong).is_err());
    }

    #[test]
    fn strict_sql_span_count_and_no_unbounded_strings() {
        let mut trace = sample();
        trace.sqlx.as_mut().unwrap().spans = vec![SqlSpan::default(); 9];
        assert_eq!(encode(&trace), Err(WireError::InvalidValue));
        let trace = sample();
        let wire = encode(&trace).unwrap();
        assert!(wire.len() <= MAX_FRAME_BYTES);
        assert!(!wire.windows(6).any(|part| part == b"SECRET"));
    }
}
