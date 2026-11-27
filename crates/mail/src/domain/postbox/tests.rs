use super::*;
use serde_json::json;

fn bounce(status: &str, subtype: &str) -> Value {
    json!({
        "eventType": "Bounce",
        "eventId": "event-1:0",
        "mail": {
            "identityId": "identity-1",
            "messageId": "message-1",
            "commonHeaders": {
                "originalMessageId": format!("<tor-{}@pandaolate.ru>", "ab".repeat(32)),
                "subject": "Do not persist me"
            }
        },
        "bounce": {
            "bounceType": "Permanent",
            "bounceSubType": subtype,
            "bouncedRecipients": [{
                "emailAddress": "reader@example.com", "status": status, "action": "failed",
                "diagnosticCode": "Do not retain me"
            }]
        }
    })
}

fn parse(value: &Value) -> Result<Option<PostboxEvent>, &'static str> {
    PostboxEvent::parse(&serde_json::to_vec(value).unwrap())
}

#[test]
fn only_explicit_unknown_mailbox_or_abuse_complaint_are_actionable() {
    let event = parse(&bounce("5.1.1", "Undetermined")).unwrap().unwrap();
    assert_eq!(event.feedback, DeliveryFeedback::RecipientDoesNotExist);
    assert_eq!(event.marker.as_deref(), Some("ab".repeat(32).as_str()));
    for code in ["5.7.1", "5.2.2", "4.4.1", "550", "5.0.0"] {
        assert!(parse(&bounce(code, "Undetermined")).unwrap().is_none());
    }
    assert!(parse(&bounce("5.1.1", "Suppressed")).unwrap().is_none());
    let mut complaint = bounce("5.1.1", "Undetermined");
    complaint["eventType"] = json!("Complaint");
    complaint["complaint"] = json!({
        "complaintFeedbackType": "abuse",
        "complainedRecipients": [{"emailAddress": "reader@example.com"}]
    });
    assert_eq!(
        parse(&complaint).unwrap().unwrap().feedback,
        DeliveryFeedback::SpamComplaint
    );
    complaint["complaint"]["complaintFeedbackType"] = json!("not-spam");
    assert!(parse(&complaint).unwrap().is_none());
}

#[test]
fn invalid_ids_markers_recipients_and_large_inputs_fail_closed() {
    let mut event = bounce("5.1.1", "Undetermined");
    event["eventId"] = json!("bad\nidentifier");
    assert!(parse(&event).is_err());
    event["eventId"] = json!("event-1");
    event["mail"]["commonHeaders"]["originalMessageId"] = json!("foreign-marker");
    assert!(parse(&event).is_err());
    event["mail"]["commonHeaders"]["originalMessageId"] = Value::Null;
    assert!(parse(&event).unwrap().unwrap().marker.is_none());
    event["bounce"]["bouncedRecipients"] = json!([
        {"emailAddress":"a@example.com","status":"5.1.1"},
        {"emailAddress":"b@example.com","status":"5.1.1"}
    ]);
    assert!(parse(&event).is_err());
    assert!(PostboxEvent::parse(&vec![b' '; MAX_EVENT_BYTES + 1]).is_err());
}
