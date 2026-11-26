//! MAIL-01g2a: bounded parser for the documented Postbox event format.
//! Parsing is NOT authentication. Call only from a verified stream consumer.
use super::suppression::DeliveryFeedback;
use serde_json::Value;

const MAX_EVENT_BYTES: usize = 65536;

pub struct PostboxEvent {
    pub(crate) identity_id: String,
    pub(crate) message_id: String,
    pub(crate) event_id: String,
    pub(crate) marker: Option<String>,
    pub(crate) recipient: String,
    pub(crate) feedback: DeliveryFeedback,
}

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

pub(crate) fn marker_token(value: &str) -> Option<&str> {
    let value = if let Some(value) = value.strip_prefix('<') {
        value.strip_suffix('>')?
    } else {
        value
    };
    let token = value.strip_prefix("tor-")?.strip_suffix("@pandaolate.ru")?;
    (token.len() == 64
        && token.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    .then_some(token)
}

fn id(value: &Value) -> Result<String, &'static str> {
    value
        .as_str()
        .filter(|value| valid_id(value))
        .map(str::to_owned)
        .ok_or("invalid Postbox identifier")
}

fn single_recipient(value: &Value) -> Result<&Value, &'static str> {
    let values = value.as_array().ok_or("missing Postbox recipient")?;
    if values.len() != 1 {
        return Err("Postbox event must describe one recipient");
    }
    Ok(&values[0])
}

impl PostboxEvent {
    /// Select only actionable, unambiguous recipient-specific feedback.
    /// Unknown kinds and generic failures are deliberately ignored.
    pub fn parse(payload: &[u8]) -> Result<Option<Self>, &'static str> {
        if payload.len() > MAX_EVENT_BYTES {
            return Err("Postbox event exceeds size limit");
        }
        let event: Value = serde_json::from_slice(payload).map_err(|_| "invalid Postbox JSON")?;
        let kind = event["eventType"]
            .as_str()
            .ok_or("missing Postbox event type")?;
        let (recipient, feedback) = match kind {
            "Bounce" => {
                let bounce = &event["bounce"];
                if bounce["bounceType"].as_str() != Some("Permanent")
                    || bounce["bounceSubType"].as_str() != Some("Undetermined")
                {
                    return Ok(None);
                }
                let recipient = single_recipient(&bounce["bouncedRecipients"])?;
                if recipient["status"].as_str() != Some("5.1.1")
                    || recipient["action"].as_str().is_some_and(|v| v != "failed")
                {
                    return Ok(None);
                }
                (recipient, DeliveryFeedback::RecipientDoesNotExist)
            }
            "Complaint" => {
                let complaint = &event["complaint"];
                if complaint["complaintFeedbackType"].as_str() != Some("abuse") {
                    return Ok(None);
                }
                let recipient = single_recipient(&complaint["complainedRecipients"])?;
                (recipient, DeliveryFeedback::SpamComplaint)
            }
            _ => return Ok(None),
        };
        let address = recipient["emailAddress"]
            .as_str()
            .filter(|v| {
                v.is_ascii()
                    && v.len() <= 254
                    && !v.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
            })
            .ok_or("invalid Postbox recipient address")?;
        let mail = &event["mail"];
        let marker = match mail["commonHeaders"]["originalMessageId"].as_str() {
            None | Some("") => None,
            Some(value) => Some(
                marker_token(value)
                    .ok_or("invalid original Postbox message marker")?
                    .to_owned(),
            ),
        };
        Ok(Some(Self {
            identity_id: id(&mail["identityId"])?,
            message_id: id(&mail["messageId"])?,
            event_id: id(&event["eventId"])?,
            marker,
            recipient: address.to_owned(),
            feedback,
        }))
    }
}

#[cfg(test)]
mod tests;
