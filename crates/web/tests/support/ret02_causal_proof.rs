//! RET-02: one explicit, read-only source -> reader outcome proof.
//! These are CI evidence labels, not new production metrics or events.
//! The parent test has already exercised real owner Save, retry/fanout and
//! two distinct authenticated end-of-text completion paths.

use super::pg;

pub(super) fn verify(
    url: &str,
    owner: i64,
    book: i64,
    released: i64,
    released_version: i64,
    reader: i64,
    observer: i64,
) {
    // The original C-15 join counted two rows but did not require the
    // ChapterPublished payload/dedupe key to match the immutable first
    // release version. This proof requires the same exact version and
    // author before any reader outcome can be attributed to that release.
    let proof = pg(
        url,
        &format!(
            r#"WITH supply AS (
                  SELECT f.chapter_id, f.version_id, p.id AS release_event_id
                  FROM chapter_first_release_events f
                  JOIN chapters chapter ON chapter.id=f.chapter_id
                    AND chapter.book_id={book}
                  JOIN outbox_events p ON p.event_type='ChapterPublished'
                    AND p.aggregate_kind='chapter' AND p.aggregate_id=f.chapter_id
                    AND p.dedupe_key=concat(
                      'chapter-published:',f.chapter_id,':',f.version_id)
                    AND p.payload->>'first_release'='true'
                    AND p.payload->>'chapter_id'=f.chapter_id::text
                    AND p.payload->>'book_id'=chapter.book_id::text
                    AND p.payload->>'version_id'=f.version_id::text
                  WHERE f.chapter_id={released}
                    AND f.version_id={released_version}
                    AND f.creator_user_id={owner}
                ), verified AS (
                  SELECT source.release_event_id, completion.user_id,
                         completion.source_event_id
                  FROM supply source
                  JOIN reader_chapter_completions completion
                    ON completion.chapter_id=source.chapter_id
                    AND completion.book_id={book}
                    AND completion.first_version_id=source.version_id
                  JOIN outbox_events event ON event.id=completion.source_event_id
                    AND event.event_type='ChapterCompletedV1'
                    AND event.aggregate_kind='chapter'
                    AND event.aggregate_id=completion.chapter_id
                    AND event.dedupe_key=concat(
                      'chapter-completed:',completion.user_id,':',completion.chapter_id)
                    AND event.payload->>'user_id'=completion.user_id::text
                    AND event.payload->>'book_id'=completion.book_id::text
                    AND event.payload->>'chapter_id'=completion.chapter_id::text
                    AND event.payload->>'version_id'=completion.first_version_id::text
                    AND event.payload->>'authority'='C-verified-free-position'
                  WHERE completion.user_id IN ({reader},{observer})
                )
                SELECT format(
                  'SUPPLY_FULFILLED=%s;READER_FULFILLED=%s;UNIQUE_READERS=%s;DUPLICATE_RELEASES=%s;DUPLICATE_READERS=%s;DUPLICATE_COMPLETIONS=%s',
                  (SELECT count(*) FROM supply),
                  (SELECT count(*) FROM verified),
                  (SELECT count(DISTINCT user_id) FROM verified),
                  (SELECT count(*)-count(DISTINCT release_event_id) FROM supply),
                  (SELECT count(*)-count(DISTINCT user_id) FROM verified),
                  (SELECT count(*)-count(DISTINCT source_event_id) FROM verified))"#
        ),
    );
    assert_eq!(
        proof,
        "SUPPLY_FULFILLED=1;READER_FULFILLED=2;UNIQUE_READERS=2;DUPLICATE_RELEASES=0;DUPLICATE_READERS=0;DUPLICATE_COMPLETIONS=0",
        "one exact-version author release must cause two unique C-verified outcomes"
    );
    // Aggregate evidence only: no actor identifiers, chapter text or secrets.
    eprintln!("RET02_CAUSAL_PROOF {proof}");
}
