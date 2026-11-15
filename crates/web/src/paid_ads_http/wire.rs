use axum::{extract::rejection::JsonRejection, http::StatusCode};

pub(super) fn id(value: &str) -> Result<i64, StatusCode> {
    if value.is_empty()
        || value.len() > 19
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)
}
pub(super) fn utc_day(value: &str) -> Result<String, StatusCode> {
    let t = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    if t.timestamp() % 86400 != 0 || t.timestamp_subsec_nanos() != 0 {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    Ok(t.with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}
pub(super) fn json_status(e: JsonRejection) -> super::ApiError {
    let status = match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::BAD_REQUEST | StatusCode::UNSUPPORTED_MEDIA_TYPE => StatusCode::BAD_REQUEST,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    if e.body_text().contains("unknown field") {
        super::coded(status, "UNKNOWN_FIELD")
    } else {
        status.into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_are_lossless_canonical_decimal_strings() {
        assert_eq!(id("9223372036854775807"), Ok(i64::MAX));
        for s in ["", "0", "01", "-1", "+1", "1e3", "9223372036854775808"] {
            assert!(id(s).is_err());
        }
    }
    #[test]
    fn dates_are_real_utc_day_boundaries_without_fractional_rounding() {
        assert_eq!(
            utc_day("2026-11-16T05:00:00+05:00").unwrap(),
            "2026-11-16T00:00:00Z"
        );
        for v in [
            "2026-11-16",
            "2026-11-16T00:00:00.000000001Z",
            "2026-11-16T00:01:00Z",
        ] {
            assert!(utc_day(v).is_err());
        }
    }
}
