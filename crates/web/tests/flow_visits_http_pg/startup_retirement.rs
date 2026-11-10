//! Read-only assertions around the actual production supervisor.
use super::pg;
use tor_books::repositories::PgCatalog;
use tor_reading::{repositories::PgReadingSync, services::progress::ReadingPlace};

pub(super) struct Probe {
    actor: i64,
    pub(super) book: i64,
    chapter: i64,
    event: String,
}
pub(super) struct Late {
    chapter: i64,
    evidence: String,
}

impl Probe {
    pub(super) fn load(url: &str) -> Self {
        let actor = pg(
            url,
            "SELECT id FROM users WHERE email_normalized='retirement-main@ci.invalid'",
        )
        .parse()
        .unwrap();
        let book = pg(
            url,
            "SELECT id FROM books WHERE source_identity='ci:retirement:linked'",
        )
        .parse()
        .unwrap();
        let chapter = pg(
            url,
            &format!("SELECT id FROM chapters WHERE book_id={book} AND chapter_no=1"),
        )
        .parse()
        .unwrap();
        let event = pg(
            url,
            "SELECT id::text FROM outbox_events WHERE dedupe_key='ci:retirement:canonical'",
        );
        Self {
            actor,
            book,
            chapter,
            event,
        }
    }

    pub(super) fn canonical(&self, url: &str) -> Vec<String> {
        let mut result = Vec::new();
        for table in [
            "reader_chapter_starts",
            "reader_library_items",
            "reader_book_follows",
        ] {
            let original_chapter = if table == "reader_chapter_starts" {
                format!(" AND chapter_id={}", self.chapter)
            } else {
                String::new()
            };
            result.push(pg(
                url,
                &format!(
                    "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]')::text
                 FROM {table} t WHERE user_id={} AND book_id={}{original_chapter}",
                    self.actor, self.book
                ),
            ));
        }
        result.push(pg(
            url,
            &format!(
                "SELECT jsonb_build_array(id,event_type,schema_version,aggregate_kind,aggregate_id,
                dedupe_key,payload,created_at,attempts,next_attempt_at,lease_epoch,dead_at)::text
             FROM outbox_events WHERE id='{}'",
                self.event
            ),
        ));
        result.push(pg(
            url,
            &format!(
                "SELECT to_jsonb(d)::text FROM event_deliveries d
             WHERE event_id='{}' AND consumer='unrelated-v1'",
                self.event
            ),
        ));
        result
    }

    pub(super) fn preferences(&self, url: &str) -> Vec<String> {
        [
            "rec_preference_accounts",
            "rec_book_preferences",
            "rec_user_preference_commands",
        ]
        .into_iter()
        .map(|table| {
            pg(
                url,
                &format!(
                    "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]')::text
                 FROM {table} t WHERE user_id={}",
                    self.actor
                ),
            )
        })
        .collect()
    }

    pub(super) async fn accept_late_c(&self, url: &str) -> Late {
        let chapter: i64 = pg(
            url,
            &format!(
                "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
             VALUES({},2,'Canonical after retirement','public','free') RETURNING id",
                self.book
            ),
        )
        .parse()
        .unwrap();
        let version: i64 = pg(
            url,
            &format!(
                "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'abcdefghij') RETURNING id"
            ),
        )
        .parse()
        .unwrap();
        pg(
            url,
            &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
        );
        let revision: i64 = pg(
            url,
            &format!(
                "SELECT COALESCE((SELECT revision FROM reader_book_positions
             WHERE user_id={} AND book_id={}),0)",
                self.actor, self.book
            ),
        )
        .parse()
        .unwrap();
        let catalog = PgCatalog::connect(url).await.unwrap();
        PgReadingSync::new(catalog.pool().clone())
            .with_flow_collection(true)
            .save_free(
                self.actor,
                chapter,
                version,
                ReadingPlace {
                    block_index: 1,
                    char_offset: 2,
                },
                revision,
            )
            .await
            .unwrap();
        catalog.pool().close().await;
        assert_eq!(
            pg(
                url,
                &format!(
                    "SELECT count(*) FROM reader_book_positions WHERE user_id={}
             AND book_id={} AND chapter_id={chapter} AND revision={revision}+1",
                    self.actor, self.book
                )
            ),
            "1"
        );
        let evidence = self.late_evidence(url, chapter);
        assert!(!evidence.is_empty());
        assert_eq!(
            pg(
                url,
                &format!(
                    "SELECT count(*) FROM reader_chapter_starts WHERE user_id={}
             AND chapter_id={}",
                    self.actor, self.chapter
                )
            ),
            "1",
            "the original canonical start stays intact"
        );
        Late { chapter, evidence }
    }

    fn late_evidence(&self, url: &str, chapter: i64) -> String {
        pg(
            url,
            &format!(
                "SELECT jsonb_build_array(to_jsonb(c),e.id,e.payload,e.created_at,
                 e.dedupe_key,e.event_type,e.schema_version)::text
             FROM reader_chapter_starts c JOIN outbox_events e ON e.id=c.source_event_id
             WHERE c.user_id={} AND c.chapter_id={chapter}",
                self.actor
            ),
        )
    }

    pub(super) fn assert_late_c(&self, url: &str, late: &Late) {
        assert_eq!(self.late_evidence(url, late.chapter), late.evidence);
        assert_eq!(pg(url, &format!(
            "SELECT count(*) FROM event_deliveries d JOIN reader_chapter_starts c ON c.source_event_id=d.event_id
             WHERE c.user_id={} AND c.chapter_id={}
               AND d.consumer IN ('recommendation-feedback-v1','flow-read-attribution-v1')",
            self.actor, late.chapter
        )), "0");
        assert_eq!(pg(url, &format!(
            "SELECT concat_ws('|',cardinality(e.required_consumers),(e.delivered_at IS NOT NULL)::int)
             FROM reader_chapter_starts c JOIN outbox_events e ON e.id=c.source_event_id
             WHERE c.user_id={} AND c.chapter_id={}", self.actor, late.chapter
        )), "0|1");
    }
}

pub(super) fn raw(url: &str) -> String {
    pg(url,
        "SELECT concat_ws('|',
          (SELECT count(*) FROM rec_subjects),(SELECT count(*) FROM rec_assignments),
          (SELECT count(*) FROM flow_visits),(SELECT count(*) FROM flow_book_cohorts),
          (SELECT count(*) FROM reader_flow_read_receipts),
          (SELECT count(*) FROM reader_activity_days),(SELECT count(*) FROM reader_return_subjects),
          (SELECT count(*) FROM reader_book_return_subjects),(SELECT count(*) FROM ril_behavior_consents),
          (SELECT count(*) FROM event_deliveries
            WHERE consumer IN ('recommendation-feedback-v1','flow-read-attribution-v1')))"
    )
}

pub(super) fn drained(url: &str) -> bool {
    raw(url) == "0|0|0|0|0|0|0|0|0|0"
        && pg(
            url,
            "SELECT concat_ws('|',(SELECT count(*) FROM rec_dirty_buckets),
              (SELECT count(*) FROM organic_discovery_dirty),
              (SELECT count(*) FROM outbox_events WHERE required_consumers &&
                ARRAY['recommendation-feedback-v1','flow-read-attribution-v1']),
              (SELECT count(*) FROM ril_flush_transitions),
              (SELECT count(*) FROM ril_flush_flow
                WHERE opted_in_html_requests>0 OR observed_opted_in_visit_starts>0))",
        ) == "0|0|0|0|0"
}

pub(super) fn assert_aggregates(url: &str, fixture: &Probe) {
    assert_eq!(
        pg(url, "SELECT sum(visits)::bigint FROM flow_visit_daily"),
        "4"
    );
    assert_eq!(
        pg(url, "SELECT sum(cohorts)::bigint FROM flow_book_daily"),
        "2"
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT string_agg(DISTINCT source,',' ORDER BY source)
         FROM rec_analytics_invalidations WHERE book_id={}",
                fixture.book
            )
        ),
        "catalog,recommendations,unattributed"
    );
    assert_eq!(
        pg(
            url,
            &format!(
                "SELECT count(*) FROM rec_quality_daily WHERE book_id={}",
                fixture.book
            )
        ),
        "0"
    );
    assert_eq!(pg(url, &format!(
        "SELECT starts FROM organic_discovery_quality_daily WHERE book_id={} AND source='search'", fixture.book
    )), "3");
    assert_eq!(
        pg(
            url,
            "SELECT d.impressions FROM rec_quality_daily d JOIN books b ON b.id=d.book_id
         WHERE b.source_identity='ci:retirement:untouched'"
        ),
        "13"
    );
}
