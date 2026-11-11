use sqlx::{postgres::PgConnectOptions, PgPool};
use std::{str::FromStr, time::{SystemTime, UNIX_EPOCH}};
use tor_accounts::{repositories::PgAccounts, services::security::{hash_password, session_token_digest}};
use tor_reading::{repositories::PgReadingSync, services::progress::ReadingPlace};
use tor_recommendations::repositories::surprises::PgSurprises;

pub const BODY: &str = "This is the first paragraph of a disposable test chapter.\n\nThe end.";

#[derive(Clone, Copy)]
pub struct Book {
    pub id: i64,
    pub chapter: i64,
    pub version: i64,
}

pub struct Fixture {
    pub pool: PgPool,
    pub surprises: PgSurprises,
    pub reading: PgReadingSync,
    pub digest: [u8; 32],
    pub users: Vec<i64>,
    pub books: Vec<Book>,
    pub run: i64,
    pub prefix: String,
}

impl Fixture {
    pub async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci URL");
        let options = PgConnectOptions::from_str(&url).unwrap();
        assert_eq!(options.get_host(), "127.0.0.1");
        assert_eq!(options.get_database(), Some("tor_ci"));
        assert_eq!(options.get_username(), "tor_app");
        let pool = PgPool::connect(&url).await.unwrap();
        sqlx::query("UPDATE surprise_runtime_policy SET enabled=false,experiment_enabled=false WHERE singleton")
            .execute(&pool).await.unwrap();
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let prefix = format!("surprise-reading-{nonce}");
        let digest = session_token_digest(&prefix);
        let admin = PgAccounts::from_pool(pool.clone()).register_with_session(
            &format!("{prefix}-admin@tor-ci.example"), "Experiment acceptance",
            &hash_password("SurpriseExperimentAcceptance2026").unwrap(), &digest,
            std::num::NonZeroU32::new(3600).unwrap(),
        ).await.unwrap();
        sqlx::query("INSERT INTO account_roles(user_id,role,is_active) VALUES($1,'administrator',true)")
            .bind(admin).execute(&pool).await.unwrap();
        Self {
            surprises: PgSurprises::new(pool.clone()),
            reading: PgReadingSync::new(pool.clone()),
            pool, digest, users: vec![admin], books: Vec::new(), run: 0, prefix,
        }
    }

    pub async fn user(&mut self) -> i64 {
        let id = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'Experiment reader','$argon2id$synthetic') RETURNING id",
        ).bind(format!("{}-{}@tor-ci.example", self.prefix, self.users.len()))
            .fetch_one(&self.pool).await.unwrap();
        self.users.push(id);
        id
    }

    pub async fn book(&mut self) -> Book {
        let id = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES($1,'ru','Experiment fixture','public','basic') RETURNING id",
        ).bind(format!("ci:{}:{}", self.prefix, self.books.len()))
            .fetch_one(&self.pool).await.unwrap();
        let chapter = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
             VALUES($1,1,'Chapter','public','free') RETURNING id",
        ).bind(id).fetch_one(&self.pool).await.unwrap();
        let version = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,$2) RETURNING id",
        ).bind(chapter).bind(BODY).fetch_one(&self.pool).await.unwrap();
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(chapter).bind(version).execute(&self.pool).await.unwrap();
        let book = Book { id, chapter, version };
        self.books.push(book);
        book
    }

    pub async fn start(&mut self) {
        self.surprises.admin_policy(&self.digest, true, 1000, true).await.unwrap();
        self.run = sqlx::query_scalar("SELECT experiment_run_id FROM surprise_runtime_policy WHERE singleton")
            .fetch_one(&self.pool).await.unwrap();
    }

    pub async fn enroll(&self, user: i64, arm: &str) {
        let assigned: String = sqlx::query_scalar(
            "SELECT arm FROM surprise_enroll($1,$2,((now() AT TIME ZONE 'UTC')::date-DATE '1970-01-01')::bigint)",
        ).bind(user).bind(arm).fetch_one(&self.pool).await.unwrap();
        assert_eq!(assigned, arm);
    }

    pub async fn advance(&self, user: i64, book: Book, block: i32, offset: i32, revision: i64) {
        self.reading.save_free(user, book.chapter, book.version,
            ReadingPlace { block_index: block, char_offset: offset }, revision).await.unwrap();
    }

    pub async fn bits(&self, user: i64) -> (bool, bool, bool, bool, bool) {
        sqlx::query_as("SELECT meaningful_new_book,current_completed,returned_d1,returned_d7,disabled
            FROM surprise_experiment_participants WHERE run_id=$1 AND user_id=$2")
            .bind(self.run).bind(user).fetch_one(&self.pool).await.unwrap()
    }

    pub async fn cleanup(self) {
        sqlx::query("UPDATE surprise_runtime_policy SET enabled=false,experiment_enabled=false WHERE singleton")
            .execute(&self.pool).await.unwrap();
        let strings: Vec<String> = self.users.iter().map(ToString::to_string).collect();
        sqlx::query("DELETE FROM event_deliveries WHERE event_id IN
            (SELECT id FROM outbox_events WHERE payload->>'user_id'=ANY($1::text[]))")
            .bind(&strings).execute(&self.pool).await.unwrap();
        for table in ["reader_chapter_completions", "reader_chapter_starts", "reader_book_positions",
            "reader_purchased_positions", "surprise_admin_events", "account_roles"] {
            let key = if table == "surprise_admin_events" { "actor_user_id" } else { "user_id" };
            sqlx::query(&format!("DELETE FROM {table} WHERE {key}=ANY($1::bigint[])"))
                .bind(&self.users).execute(&self.pool).await.unwrap();
        }
        sqlx::query("DELETE FROM outbox_events WHERE payload->>'user_id'=ANY($1::text[])")
            .bind(&strings).execute(&self.pool).await.unwrap();
        let books: Vec<i64> = self.books.iter().map(|book| book.id).collect();
        sqlx::query("DELETE FROM books WHERE id=ANY($1::bigint[])")
            .bind(&books).execute(&self.pool).await.unwrap();
        sqlx::query("DELETE FROM users WHERE id=ANY($1::bigint[])")
            .bind(&self.users).execute(&self.pool).await.unwrap();
    }
}
