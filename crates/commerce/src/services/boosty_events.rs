//! D-10: Boosty subscription events are observations, not settled payments.
//! This module never constructs VerifiedSubscriptionCharge or moves wallet money.
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_PAGE_ROWS: usize = 100;
pub const MAX_RESPONSE_BYTES: usize = 1_048_576;

#[derive(Debug, Eq, PartialEq)]
pub enum ObserveError {
    InvalidConfig,
    Network,
    HttpStatus(u16),
    ResponseTooLarge,
    InvalidResponse,
    Pagination,
}

impl std::fmt::Display for ObserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig => write!(f, "invalid observer configuration"),
            Self::Network => write!(f, "Boosty request failed"),
            Self::HttpStatus(code) => write!(f, "Boosty returned HTTP {code}"),
            Self::ResponseTooLarge => write!(f, "Boosty response exceeded size limit"),
            Self::InvalidResponse => write!(f, "unexpected Boosty events response"),
            Self::Pagination => write!(f, "Boosty pagination is incomplete or unsafe"),
        }
    }
}

impl std::error::Error for ObserveError {}

#[derive(Debug, Serialize)]
pub struct CandidateField {
    pub field: &'static str,
    /// Diagnostic checksum only, NOT a provider payment identifier.
    pub sha256_prefix: String,
}

#[derive(Debug, Serialize)]
pub struct ObservedEvent {
    pub event_type: &'static str,
    pub created_at: Option<i64>,
    /// Provider's uninterpreted amount, not assumed to be RUB kopecks.
    pub amount_raw: Option<String>,
    pub amount_field_present: bool,
    pub currency_raw: Option<String>,
    pub fee_paid_flag: Option<bool>,
    pub has_payer_id: bool,
    pub candidate_identity_fields: Vec<CandidateField>,
    /// Changes when any field changes. Never use it as a credit dedupe key.
    pub snapshot_sha256_prefix: String,
}

#[derive(Debug)]
pub struct EventPage {
    pub events: Vec<ObservedEvent>,
    pub rows_examined: usize,
    pub is_last: bool,
    pub next_offset: Option<String>,
}

fn fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn candidate_fields(value: &Value) -> Vec<CandidateField> {
    const FIELDS: [&str; 12] = [
        "id",
        "eventId",
        "event_id",
        "operationId",
        "operation_id",
        "paymentId",
        "payment_id",
        "transactionId",
        "transaction_id",
        "saleId",
        "sale_id",
        "chargeId",
    ];
    FIELDS
        .into_iter()
        .filter_map(|field| {
            let raw = match value.get(field)? {
                Value::String(s) if !s.is_empty() && s.len() <= 120 => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some(CandidateField {
                field,
                sha256_prefix: fingerprint(raw.as_bytes()),
            })
        })
        .collect()
}

fn observe(value: &Value) -> Option<ObservedEvent> {
    let kind = match value.get("eventType")?.as_str()? {
        "subscribe" => "subscribe",
        "recurrent" => "recurrent",
        _ => return None,
    };
    let amount = value.get("amount");
    let amount_raw = match amount {
        Some(Value::Number(number)) => Some(number.to_string()),
        Some(Value::String(text)) if text.len() <= 32 => Some(text.clone()),
        _ => None,
    };
    let currency = value
        .get("currency")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= 5 && s.bytes().all(|b| b.is_ascii_uppercase()));
    Some(ObservedEvent {
        event_type: kind,
        created_at: value.get("createdAt").and_then(Value::as_i64),
        amount_raw,
        amount_field_present: amount.is_some(),
        currency_raw: currency.map(str::to_owned),
        fee_paid_flag: value.get("isFeePaid").and_then(Value::as_bool),
        has_payer_id: value
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(Value::as_i64)
            .is_some_and(|id| id > 0),
        candidate_identity_fields: candidate_fields(value),
        snapshot_sha256_prefix: fingerprint(value.to_string().as_bytes()),
    })
}

/// Never return emails, names, numeric payer IDs, tokens or raw arbitrary rows.
pub fn parse_events(body: &[u8]) -> Result<EventPage, ObserveError> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(ObserveError::ResponseTooLarge);
    }
    let payload: Value = serde_json::from_slice(body).map_err(|_| ObserveError::InvalidResponse)?;
    let data = payload
        .get("data")
        .and_then(Value::as_array)
        .ok_or(ObserveError::InvalidResponse)?;
    if data.len() > MAX_PAGE_ROWS || data.iter().any(|row| !row.is_object()) {
        return Err(ObserveError::InvalidResponse);
    }
    let extra = payload.get("extra").and_then(Value::as_object);
    let is_last = extra
        .and_then(|m| m.get("isLast"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next_offset = extra
        .and_then(|m| m.get("offset"))
        .and_then(|cursor| match cursor {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
    if next_offset.as_deref().is_some_and(|s| !valid_cursor(s)) {
        return Err(ObserveError::Pagination);
    }
    Ok(EventPage {
        events: data.iter().filter_map(observe).collect(),
        rows_examined: data.len(),
        is_last,
        next_offset,
    })
}

pub fn valid_cursor(s: &str) -> bool {
    !s.is_empty() && s.len() <= 19 && s.bytes().all(|b| b.is_ascii_digit())
}

pub fn valid_blog(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_10_probe_observes_renewals_without_leaking_payer_identity() {
        let data = br#"{"data":[
          {"eventType":"subscribe","id":"event-first","createdAt":1790000000,
           "amount":150.5,"isFeePaid":false,
           "user":{"id":42,"email":"private@example.test","name":"Secret"}},
          {"eventType":"recurrent","paymentId":"renewal-2","amount":180,
           "user":{"id":42,"email":"private@example.test"}},
          {"eventType":"unsubscribe","user":{"id":42}}
        ],"extra":{"offset":"3","isLast":false}}"#;
        let page = parse_events(data).unwrap();
        assert_eq!(page.rows_examined, 3);
        assert_eq!(page.events.len(), 2);
        assert_eq!(page.events[0].event_type, "subscribe");
        assert_eq!(page.events[1].event_type, "recurrent");
        assert_eq!(page.events[0].amount_raw.as_deref(), Some("150.5"));
        assert_eq!(page.events[0].candidate_identity_fields[0].field, "id");
        assert_eq!(page.next_offset.as_deref(), Some("3"));
        assert!(!page.is_last);
        let json = serde_json::to_string(&page.events).unwrap();
        for private in ["private@example.test", "Secret", "event-first", "renewal-2"] {
            assert!(!json.contains(private));
        }
    }

    #[test]
    fn d_10_probe_rejects_untrusted_pagination_and_oversized_payload() {
        assert!(!valid_blog("../other"));
        assert!(!valid_blog("blog?limit=300"));
        assert!(valid_blog("my_blog-1"));
        assert!(!valid_cursor("1&event_types=payout"));
        assert_eq!(
            parse_events(br#"{"data":[],"extra":{"offset":"a?b"}}"#).unwrap_err(),
            ObserveError::Pagination
        );
        assert_eq!(
            parse_events(br#"{"data":{}}"#).unwrap_err(),
            ObserveError::InvalidResponse
        );
        assert_eq!(
            parse_events(&vec![b'x'; MAX_RESPONSE_BYTES + 1]).unwrap_err(),
            ObserveError::ResponseTooLarge
        );
    }

    #[test]
    fn d_10_probe_does_not_treat_arbitrary_amount_as_rub_kopecks() {
        let page = parse_events(
            br#"{"data":[
          {"eventType":"recurrent","amount":"unknown","user":{"id":1}}
        ],"extra":{"isLast":true}}"#,
        )
        .unwrap();
        assert_eq!(page.events[0].amount_raw.as_deref(), Some("unknown"));
        assert!(page.events[0].candidate_identity_fields.is_empty());
        assert!(page.is_last);
    }
}
