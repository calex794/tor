use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    Client,
};
use sha2::{Digest, Sha256};
use std::{env, process::Command, sync::Arc};
use tor_accounts::services::security::generate_session_token;
use tor_books::{
    repositories::{free_s3_publish::PgFreeS3Publication, PgCatalog},
    services::chapter_file_import::{fingerprint, parse_text},
};
use tor_imports::{
    repositories::{postgres::PgImportJobs, s3::S3ChapterImportStore},
    services::ChapterImportObjectStore,
};
use tor_storage::repositories::free_chapter_s3::S3FreeChapterStore;
use tor_web::{
    chapter_import_worker::ChapterImportWorker,
    free_author_publish::{PgFreeAuthorPublisher, S3FreeAuthor},
};

pub const BUCKET: &str = "tor-drop-worker-ci";
pub fn pg(url: &str, sql: &str) -> String {
    assert!(url.contains("@127.0.0.1:5432/") && url.ends_with("/tor_drop_worker_ci"));
    let output = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            sql,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "PG fixture: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub struct Fixture {
    pub url: String,
    pub client: Client,
    pub catalog: PgCatalog,
    pub actor: i64,
    pub book: i64,
    pub source: Arc<S3ChapterImportStore>,
}
impl Fixture {
    pub async fn new(label: &str) -> Self {
        let url = env::var("TEST_DATABASE_URL").unwrap();
        assert!(url.ends_with("/tor_drop_worker_ci"));
        assert_eq!(
            env::var("TOR_TEST_S3_ENDPOINT").unwrap(),
            "http://127.0.0.1:9000"
        );
        let client = Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version(BehaviorVersion::latest())
                .region(Region::new("us-east-1"))
                .credentials_provider(Credentials::new(
                    env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                    env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                    None,
                    None,
                    "drop-worker-test",
                ))
                .endpoint_url("http://127.0.0.1:9000")
                .force_path_style(true)
                .retry_config(RetryConfig::standard().with_max_attempts(1))
                .build(),
        );
        let _ = client.create_bucket().bucket(BUCKET).send().await;
        let id = generate_session_token();
        let actor = pg(&url, &format!("INSERT INTO users(email_normalized,display_name,password_hash) VALUES('{id}@tor.example','DROP worker','$argon2id$test') RETURNING id")).parse().unwrap();
        let book = pg(&url, &format!("INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) VALUES('ci:drop-worker:{id}','ru','{label}','draft',{actor}) RETURNING id")).parse().unwrap();
        let catalog = PgCatalog::connect(&url).await.unwrap();
        let source = Arc::new(S3ChapterImportStore::new(client.clone(), BUCKET.into()).unwrap());
        Self {
            url,
            client,
            catalog,
            actor,
            book,
            source,
        }
    }
    pub fn jobs(&self) -> PgImportJobs {
        PgImportJobs::new(self.catalog.pool().clone())
    }
    pub fn author(&self) -> S3FreeAuthor {
        PgFreeAuthorPublisher::new(
            PgFreeS3Publication::new(self.catalog.pool().clone()),
            S3FreeChapterStore::new(self.client.clone(), BUCKET.into()).unwrap(),
            BUCKET.into(),
        )
        .unwrap()
    }
    pub fn worker(&self, packed: bool) -> ChapterImportWorker {
        let author = self.author();
        let author = if packed {
            author.with_direct_pack_import(self.client.clone())
        } else {
            author
        };
        ChapterImportWorker::new(&self.catalog, self.source.clone()).with_free_s3(author)
    }
    pub async fn enqueue(&self, source: &str) -> (i64, String) {
        let key = generate_session_token();
        let object_key = format!("staging/chapters/{}/{}/{key}", self.actor, self.book);
        let hash: [u8; 32] = Sha256::digest(source.as_bytes()).into();
        let chapters = parse_text("chapters.txt", source).unwrap();
        self.source
            .put_verified(&object_key, source.as_bytes(), &hash)
            .await
            .unwrap();
        let job = self
            .jobs()
            .enqueue_verified(
                self.actor,
                self.book,
                &key,
                &object_key,
                "chapters.txt",
                &hash,
                source.len() as i64,
                &fingerprint(self.book, &chapters),
                chapters.len() as i32,
            )
            .await
            .unwrap();
        (job, object_key)
    }
    pub fn count(&self, table: &str) -> i64 {
        assert!([
            "chapters",
            "chapter_versions",
            "chapter_publications",
            "free_chapter_storage_objects",
            "free_edition_active_storage"
        ]
        .contains(&table));
        let query = if table == "chapters" {
            format!("SELECT count(*) FROM chapters WHERE book_id={}", self.book)
        } else {
            format!("SELECT count(*) FROM {table} t JOIN chapters c ON c.id=t.chapter_id WHERE c.book_id={}", self.book)
        };
        pg(&self.url, &query).parse().unwrap()
    }
}
