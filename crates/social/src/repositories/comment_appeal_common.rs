//! Shared appeal session locks and input validation; target-specific policies stay in their repositories.
#![forbid(unsafe_code)]
use super::book_shares::BookShareError;
use sqlx::{Postgres, Transaction};

fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(super) async fn actor(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
    filing: bool,
) -> Result<i64, BookShareError> {
    let query = if filing {
        "SELECT u.id FROM sessions s JOIN users u ON u.id=s.user_id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
           AND u.account_state='active' FOR UPDATE OF u FOR SHARE OF s"
    } else {
        "SELECT u.id FROM sessions s JOIN users u ON u.id=s.user_id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
           AND u.account_state='active' FOR SHARE OF s,u"
    };
    sqlx::query_scalar(query)
        .bind(digest.as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(BookShareError::Forbidden)
}

pub(super) fn validate_limit(limit: u16) -> Result<(), BookShareError> {
    if !(1..=50).contains(&limit) {
        return Err(BookShareError::Invalid);
    }
    Ok(())
}

pub(super) fn submission_reason<'a>(
    hide_event_id: i64,
    reason: &'a str,
    command_key: &str,
) -> Result<&'a str, BookShareError> {
    let reason = reason.trim();
    if hide_event_id <= 0
        || !(20..=1000).contains(&reason.chars().count())
        || reason.contains('\0')
        || !valid_key(command_key)
    {
        return Err(BookShareError::Invalid);
    }
    Ok(reason)
}

pub(super) fn decision_note<'a>(
    appeal_id: i64,
    decision: &str,
    note: &'a str,
) -> Result<&'a str, BookShareError> {
    let note = note.trim();
    if appeal_id <= 0
        || !matches!(decision, "denied" | "restored")
        || note.is_empty()
        || note.chars().count() > 500
        || note.contains('\0')
    {
        return Err(BookShareError::Invalid);
    }
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_and_queue_limits_are_bounded() {
        for limit in [0, 51, u16::MAX] {
            assert!(matches!(
                validate_limit(limit),
                Err(BookShareError::Invalid)
            ));
        }
        for limit in [1, 50] {
            assert!(validate_limit(limit).is_ok());
        }
    }

    #[test]
    fn submission_preserves_trimmed_unicode_and_bounds() {
        let key = "a".repeat(43);
        for count in [20, 1000] {
            let reason = "é".repeat(count);
            let padded = format!("  {reason}\n");
            assert_eq!(
                submission_reason(1, &padded, &key).unwrap(),
                reason.as_str()
            );
        }
        for count in [19, 1001] {
            let reason = "é".repeat(count);
            assert!(matches!(
                submission_reason(1, &reason, &key),
                Err(BookShareError::Invalid)
            ));
        }
    }

    #[test]
    fn submission_rejects_invalid_identity_text_and_key() {
        let reason = "a".repeat(20);
        let key = "a".repeat(43);
        for id in [0, -1] {
            assert!(matches!(
                submission_reason(id, &reason, &key),
                Err(BookShareError::Invalid)
            ));
        }
        for bad in [
            "a".repeat(42),
            "a".repeat(44),
            format!("{}=", "a".repeat(42)),
            format!("{}é", "a".repeat(41)),
        ] {
            assert!(matches!(
                submission_reason(1, &reason, &bad),
                Err(BookShareError::Invalid)
            ));
        }
        let nul = format!("{reason}\0");
        assert!(matches!(
            submission_reason(1, &nul, &key),
            Err(BookShareError::Invalid)
        ));
        assert!(submission_reason(1, &reason, &format!("{}-_", "a".repeat(41))).is_ok());
    }

    #[test]
    fn decision_requires_allowed_action_and_nonempty_bounded_note() {
        for action in ["denied", "restored"] {
            assert_eq!(decision_note(1, action, "  note  ").unwrap(), "note");
            assert!(decision_note(1, action, &"é".repeat(500)).is_ok());
        }
        for note in [" ".to_owned(), "x\0".to_owned(), "é".repeat(501)] {
            assert!(matches!(
                decision_note(1, "denied", &note),
                Err(BookShareError::Invalid)
            ));
        }
        assert!(matches!(
            decision_note(0, "denied", "note"),
            Err(BookShareError::Invalid)
        ));
        assert!(matches!(
            decision_note(1, "pending", "note"),
            Err(BookShareError::Invalid)
        ));
    }
}
