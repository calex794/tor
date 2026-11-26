//! D-001 offline physical compaction DRILL, not a runtime collector.
//! Only new non-versioned loopback MinIO buckets and fixture-owned SINGLEs.
//! Every deletion is preceded by a byte-verified copy in the drill backup
//! bucket. Restore all removed objects before ending the experiment. No
//! production database, arbitrary object keys, PACK, paid data, or live grants.
use super::*;
use tor_reading::repositories::{free_open_capture::PgFreeOpenCapture, PgLibrary};
use tor_storage::services::free_chapter::{FreeChapterObject, FreeChapterStore};
use tor_web::author_chapter_http::FreeChapterPublisher;

const PRIMARY: &str = "tor-d001-single-drill-primary-ci";
const BACKUP: &str = "tor-d001-single-drill-backup-ci";

struct Drill {
    client: Client,
    primary: S3FreeChapterStore,
    backup: S3FreeChapterStore,
}

impl Drill {
    async fn new() -> Self {
        assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
        let client = Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("us-east-1"))
                .credentials_provider(Credentials::new(
                    env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                    env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                    None,
                    None,
                    "local-d001-single-drill",
                ))
                .endpoint_url(ENDPOINT)
                .force_path_style(true)
                .retry_config(RetryConfig::standard().with_max_attempts(1))
                .build(),
        );
        for bucket in [PRIMARY, BACKUP] {
            client.create_bucket().bucket(bucket).send().await.unwrap();
            let versioning = client
                .get_bucket_versioning()
                .bucket(bucket)
                .send()
                .await
                .unwrap();
            assert!(
                versioning.status().is_none(),
                "the drill must use genuinely unversioned buckets"
            );
        }
        Self {
            primary: S3FreeChapterStore::new(client.clone(), PRIMARY.to_owned()).unwrap(),
            backup: S3FreeChapterStore::new(client.clone(), BACKUP.to_owned()).unwrap(),
            client,
        }
    }

    async fn inventory(&self, bucket: &str) -> Vec<(String, u64)> {
        assert!(matches!(bucket, PRIMARY | BACKUP));
        let result = self
            .client
            .list_objects_v2()
            .bucket(bucket)
            .send()
            .await
            .unwrap();
        assert!(!result.is_truncated().unwrap_or(false));
        let mut rows: Vec<_> = result
            .contents()
            .iter()
            .map(|object| {
                (
                    object.key().unwrap().to_owned(),
                    u64::try_from(object.size().unwrap()).unwrap(),
                )
            })
            .collect();
        rows.sort_unstable();
        rows
    }

    async fn assert_inventory(&self, bucket: &str, expected: &[FreeChapterObject]) -> u64 {
        let actual = self.inventory(bucket).await;
        let mut keys: Vec<_> = expected
            .iter()
            .map(|object| (object.key(), object.size_bytes()))
            .collect();
        keys.sort_unstable();
        assert_eq!(actual, keys);
        actual.iter().map(|(_, bytes)| bytes).sum()
    }

    // This is deliberately a fixture-only operation, with no concurrent
    // readers or writers. The D-001 views are NOT production delete permits.
    // Preserve all metadata, including completed idempotency receipts.
    async fn compact(
        &self,
        url: &str,
        chapter: i64,
        live: &mut Vec<FreeChapterObject>,
        retired: &mut Vec<FreeChapterObject>,
        keep: &[i64],
    ) {
        let before = self.assert_inventory(PRIMARY, live).await;
        let count_before = live.len();
        let ids = pg(
            url,
            &format!(
                "SELECT v.version_id FROM d001_registered_storage_overhang v
                 JOIN free_chapter_storage_objects f
                   ON f.chapter_id=v.chapter_id AND f.version_id=v.version_id
                 WHERE v.chapter_id={chapter} AND f.bucket='{PRIMARY}'
                   AND NOT v.has_active_pack_placement
                   AND NOT EXISTS (
                     SELECT 1 FROM reader_free_open_intents i
                     WHERE i.chapter_id=v.chapter_id AND i.version_id=v.version_id)
                 ORDER BY v.version_id"
            ),
        );
        let candidates: Vec<i64> = ids.lines().map(|line| line.parse().unwrap()).collect();
        let removed: Vec<_> = live
            .iter()
            .filter(|object| candidates.contains(&object.version_id()))
            .cloned()
            .collect();
        let remaining: Vec<_> = live
            .iter()
            .filter(|object| !candidates.contains(&object.version_id()))
            .cloned()
            .collect();
        assert_eq!(
            remaining
                .iter()
                .map(FreeChapterObject::version_id)
                .collect::<Vec<_>>(),
            keep,
            "the actual retained set must exactly match the fixture expectation"
        );
        // Verify every primary and backup object before issuing any DELETE.
        for object in &removed {
            assert_eq!(object.chapter_id(), chapter);
            let bytes = self.primary.get_verified(object).await.unwrap();
            self.backup.put_immutable(object, &bytes).await.unwrap();
            assert_eq!(self.backup.get_verified(object).await.unwrap(), bytes);
        }
        for object in &removed {
            self.client
                .delete_object()
                .bucket(PRIMARY)
                .key(object.key())
                .send()
                .await
                .unwrap();
            assert!(self.primary.get_verified(object).await.is_err());
        }
        *live = remaining;
        retired.extend(removed);
        let after = self.assert_inventory(PRIMARY, live).await;
        self.assert_inventory(BACKUP, retired).await;
        for object in live.iter() {
            self.primary.get_verified(object).await.unwrap();
        }
        assert!(after < before);
        writeln!(
            std::io::stdout().lock(),
            "D001_SINGLE_DRILL: primary_objects={count_before}->{} primary_bytes={before}->{after} backed_up_objects={}",
            live.len(),
            retired.len()
        )
        .unwrap();
    }

    async fn restore(&self, live: &mut Vec<FreeChapterObject>, retired: &[FreeChapterObject]) {
        for object in retired {
            let bytes = self.backup.get_verified(object).await.unwrap();
            self.primary.put_immutable(object, &bytes).await.unwrap();
            assert_eq!(self.primary.get_verified(object).await.unwrap(), bytes);
        }
        live.extend_from_slice(retired);
        self.assert_inventory(PRIMARY, live).await;
    }
}

fn current(url: &str, chapter: i64) -> i64 {
    pg(
        url,
        &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
    )
    .parse()
    .unwrap()
}

fn retained(url: &str, chapter: i64) -> String {
    pg(
        url,
        &format!(
            "SELECT logical_editions,reader_retained_editions,physical_materializations
             FROM d001_chapter_logical_counts WHERE chapter_id={chapter}"
        ),
    )
}

fn user_id(url: &str, label: &str) -> i64 {
    pg(
        url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('d001-single-drill-{label}@example.invalid','D001 drill','$argon2id$test')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PG18 and fresh loopback non-versioned MinIO"]
async fn d001_single_offline_drill_reclaims_real_bytes_and_restores_exact_editions() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("@127.0.0.1:5432/tor_ci"));
    let drill = Drill::new().await;
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let owner = user_id(&url, "owner");
    let reader_one = user_id(&url, "reader-one");
    let reader_two = user_id(&url, "reader-two");
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:d001:single-drill','ru','D001 SINGLE drill','public',{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let publication = PgFreeS3Publication::new(pool.clone());
    let writer =
        PgFreeAuthorPublisher::new(publication, drill.primary.clone(), PRIMARY.to_owned()).unwrap();
    let original = "Original body A for physical drill";
    let first = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        expected_revision: 0,
        number: 1,
        title: "D001 SINGLE",
        body: original,
        access: "free",
        price_amount: 0,
        command_key: &generate_session_token(),
    })
    .unwrap();
    writer.publish(owner, &first).await.unwrap();
    let chapter: i64 = pg(
        &url,
        &format!("SELECT id FROM chapters WHERE book_id={book} AND chapter_no=1"),
    )
    .parse()
    .unwrap();
    let first_version = current(&url, chapter);
    let mut live = vec![FreeChapterObject::for_markdown(chapter, first_version, original).unwrap()];
    let mut retired = Vec::new();
    assert_eq!(retained(&url, chapter), "1|1|1");

    let edit = |body: &str| {
        let revision: i64 = pg(
            &url,
            &format!("SELECT edit_revision FROM chapters WHERE id={chapter}"),
        )
        .parse()
        .unwrap();
        save_chapter(SaveChapterRequest {
            book_id: book as u64,
            chapter_id: Some(chapter as u64),
            expected_revision: revision,
            number: 1,
            title: "D001 SINGLE",
            body,
            access: "free",
            price_amount: 0,
            command_key: &generate_session_token(),
        })
        .unwrap()
    };
    let mut old_command = None;
    for n in 1..=10 {
        let body = format!("Unpinned edit {n}: {}", "text ".repeat(128));
        let request = edit(&body);
        writer.publish_edit(owner, &request).await.unwrap();
        live.push(FreeChapterObject::for_markdown(chapter, current(&url, chapter), &body).unwrap());
        if n == 1 {
            old_command = Some(request);
        }
    }
    assert_eq!(retained(&url, chapter), "1|1|11");
    let pinned_one = current(&url, chapter);
    drill
        .compact(&url, chapter, &mut live, &mut retired, &[pinned_one])
        .await;
    assert_eq!(live.len(), 1);
    assert_eq!(retired.len(), 10);
    // Replay an already completed save of a removed, unpinned materialization:
    // must not recreate old bytes, advance the head, or allocate a new version.
    writer
        .publish_edit(owner, &old_command.unwrap())
        .await
        .unwrap();
    assert_eq!(current(&url, chapter), pinned_one);
    drill.assert_inventory(PRIMARY, &live).await;
    assert_eq!(retained(&url, chapter), "1|1|11");

    let library = PgLibrary::new(pool.clone());
    let opened = PgFreeOpenCapture::new(pool);
    library.add_book(reader_one, book).await.unwrap();
    drill.primary.get_verified(&live[0]).await.unwrap();
    assert!(opened
        .record(reader_one, chapter, pinned_one)
        .await
        .unwrap());
    assert_eq!(retained(&url, chapter), "1|1|11");
    // Re-publish A after its earlier physical copy was removed. A fresh
    // immutable ID/key must be used, not the deleted object's key.
    for body in [
        original,
        "Unpinned logical V2 replacement",
        "Latest logical V2",
    ] {
        writer.publish_edit(owner, &edit(body)).await.unwrap();
        let version = current(&url, chapter);
        assert_ne!(version, first_version);
        live.push(FreeChapterObject::for_markdown(chapter, version, body).unwrap());
    }
    let pinned_two = current(&url, chapter);
    assert_eq!(retained(&url, chapter), "2|2|14");
    drill
        .compact(
            &url,
            chapter,
            &mut live,
            &mut retired,
            &[pinned_one, pinned_two],
        )
        .await;
    assert_eq!(live.len(), 2);

    library.add_book(reader_two, book).await.unwrap();
    drill.primary.get_verified(&live[1]).await.unwrap();
    assert!(opened
        .record(reader_two, chapter, pinned_two)
        .await
        .unwrap());
    for body in ["Intermediate logical V3", "Current logical V3"] {
        writer.publish_edit(owner, &edit(body)).await.unwrap();
        live.push(FreeChapterObject::for_markdown(chapter, current(&url, chapter), body).unwrap());
    }
    let head = current(&url, chapter);
    assert_eq!(retained(&url, chapter), "3|3|16");
    drill
        .compact(
            &url,
            chapter,
            &mut live,
            &mut retired,
            &[pinned_one, pinned_two, head],
        )
        .await;
    assert_eq!(live.len(), 3);
    assert_eq!(retired.len(), 13);
    for (reader, version) in [(reader_one, pinned_one), (reader_two, pinned_two)] {
        assert_eq!(
            pg(
                &url,
                &format!(
                    "SELECT version_id FROM reader_saved_free_chapters
                     WHERE user_id={reader} AND chapter_id={chapter}"
                ),
            ),
            version.to_string()
        );
    }
    // The drill backup is on the same MinIO, NOT independent disaster recovery.
    // Old metadata/administrative history still references primary keys. Restore
    // every old object before ending; do not pretend those references are safe
    // after an online deletion or authorize a runtime collector from this test.
    drill.restore(&mut live, &retired).await;
    assert_eq!(live.len(), 16);
    for object in &live {
        drill.primary.get_verified(object).await.unwrap();
    }
    assert_eq!(retained(&url, chapter), "3|3|16");
    writeln!(
        std::io::stdout().lock(),
        "D001_SINGLE_DRILL_RESTORE_OK: 16/16 exact primary objects restored"
    )
    .unwrap();
}
