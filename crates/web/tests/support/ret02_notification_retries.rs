//! RET-02b: PostgreSQL release notification retry and fenced-lease acceptance.
//! Isolated tor_ci state is created by the parent C-15 HTTP/PG/Chromium test.
//! A simulated lease/retry is not a claim of tested external MinIO failures.
use super::pg;
use tor_social::repositories::notifications::PgNotifications;

pub(super) async fn verify(url: &str, released: i64, reader: i64, notices: &PgNotifications) {
    // RET-02b: a fenced root from an interrupted worker cannot fan out.
    // Both the lease and retry deadline must mature before projection.
    let release_root = format!(
        "event_type='ChapterPublished' AND aggregate_id={released}
         AND payload->>'first_release'='true'"
    );
    pg(
        url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()+interval '10 minutes',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_root}"
        ),
    );
    let _ = notices.fanout_chapter_releases(32).await.unwrap();
    let release_children = || {
        pg(
            url,
            &format!(
                "SELECT count(*) FROM outbox_events
                 WHERE event_type='reader.chapter.released' AND aggregate_id={released}"
            ),
        )
    };
    assert_eq!(release_children(), "0", "an active lease blocks fanout");
    pg(
        url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()+interval '10 minutes'
             WHERE {release_root}"
        ),
    );
    let _ = notices.fanout_chapter_releases(32).await.unwrap();
    assert_eq!(release_children(), "0", "retry deadline blocks premature fanout");
    pg(
        url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_root}"
        ),
    );
    for _ in 0..8 {
        if notices.fanout_chapter_releases(32).await.unwrap() == 0 {
            break;
        }
    }
    // E-08 child delivery has its own fenced lease and independent retry.
    // A root release must never become a duplicate personal bell on replay.
    let release_child = format!(
        "event_type='reader.chapter.released' AND aggregate_id={released}
         AND payload->>'recipient_user_id'='{reader}'"
    );
    assert_eq!(
        pg(url, &format!("SELECT count(*) FROM outbox_events WHERE {release_child}")),
        "1",
        "one child event was created for the follower"
    );
    let delivered_bells = || {
        pg(
            url,
            &format!(
                "SELECT count(*) FROM user_notifications n
                 JOIN outbox_events e ON e.id=n.source_event_id
                 WHERE {release_child}"
            ),
        )
    };
    pg(
        url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()+interval '10 minutes',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_child}"
        ),
    );
    let _ = notices.deliver_pending(64).await.unwrap();
    assert_eq!(delivered_bells(), "0", "a fenced child cannot notify");
    pg(
        url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()+interval '10 minutes'
             WHERE {release_child}"
        ),
    );
    let _ = notices.deliver_pending(64).await.unwrap();
    assert_eq!(delivered_bells(), "0", "a deferred child cannot notify early");
    pg(
        url,
        &format!(
            "UPDATE outbox_events
             SET lease_until=clock_timestamp()-interval '1 second',
                 next_attempt_at=clock_timestamp()-interval '1 second'
             WHERE {release_child}"
        ),
    );
    for _ in 0..8 {
        if notices.deliver_pending(64).await.unwrap() == 0 {
            break;
        }
    }
    // Repeated fanout and delivery can be retried after an uncertain ACK,
    // but must preserve one child event and one durable notification.
    let _ = notices.fanout_chapter_releases(32).await.unwrap();
    let _ = notices.deliver_pending(64).await.unwrap();
    assert_eq!(release_children(), "1", "replay created another child event");
    assert_eq!(delivered_bells(), "1", "replay created another personal bell");
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM event_deliveries d
                 JOIN outbox_events e ON e.id=d.event_id
                 WHERE {release_child} AND d.consumer='user_notifications'"
            ),
        ),
        "1",
        "a single child event must have one durable delivery receipt"
    );
}
