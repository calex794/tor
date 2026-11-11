//! FLOW-08: actual nonfinancial campaigns, current authority and three safe slots.
#![forbid(unsafe_code)]
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tor_ads::{
    domain::editorial::*, repositories::editorial::PgEditorial, services::editorial::Editorial,
};
use tor_contracts::exposure::{CardObservation, EditorialPlacement, PromotionSlot};

struct Fixture {
    pool: PgPool,
    store: PgEditorial,
    admin: Administrator,
    stamp: u128,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
        assert!(url.starts_with("postgresql://tor_app:") && url.contains("@127.0.0.1:5432/tor_ci"));
        let pool = PgPoolOptions::new()
            .max_connections(6)
            .acquire_timeout(Duration::from_secs(2))
            .connect(&url)
            .await
            .unwrap();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let admin = user(&pool, &format!("editorial-{stamp}"), true).await;
        Self {
            store: PgEditorial::new(pool.clone()),
            pool,
            admin: Administrator {
                user_id: admin.user_id,
                session_digest: admin.session_digest,
            },
            stamp,
        }
    }
    async fn book(&self, label: &str, access: &str) -> i64 {
        sqlx::query_scalar("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES($1,'ru',$2,'public',$3) RETURNING id")
            .bind(format!("ci:editorial:{}:{label}",self.stamp)).bind(format!("Editorial <safe> {label}"))
            .bind(access).fetch_one(&self.pool).await.unwrap()
    }
    async fn create(&self, book: i64, slot: PromotionSlot, key: u32) -> Receipt {
        Editorial::new(&self.store)
            .execute(&self.admin, &key_string(key), &create(book, slot))
            .await
            .unwrap()
    }
    async fn act(&self, r: &Receipt, action: Action, key: u32) -> Result<Receipt, EditorialError> {
        Editorial::new(&self.store)
            .execute(
                &self.admin,
                &key_string(key),
                &Command {
                    action,
                    campaign_id: Some(r.campaign_id),
                    expected_revision: r.revision,
                    details: None,
                },
            )
            .await
    }
    async fn active(&self, book: i64, slot: PromotionSlot, key: u32) -> Receipt {
        let draft = self.create(book, slot, key).await;
        self.act(&draft, Action::Activate, key + 1).await.unwrap()
    }
    async fn clean(self) {
        sqlx::query("DELETE FROM books WHERE source_identity LIKE $1")
            .bind(format!("ci:editorial:{}:%", self.stamp))
            .execute(&self.pool)
            .await
            .unwrap();
        // Administrative receipts remain immutable even after source deletion.
        sqlx::query("DELETE FROM account_roles WHERE user_id=$1")
            .bind(self.admin.user_id)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.admin.user_id)
            .execute(&self.pool)
            .await
            .unwrap();
        self.pool.close().await;
    }
}
async fn user(pool: &PgPool, name: &str, administrator: bool) -> Viewer {
    let id:i64=sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'Editorial fixture','$argon2id$fixture') RETURNING id")
        .bind(format!("{name}@example.invalid")).fetch_one(pool).await.unwrap();
    let digest: [u8; 32] =
        Sha256::digest(format!("editorial-session:{id}:{name}").as_bytes()).into();
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(digest.as_slice())
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    if administrator {
        sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'administrator')")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
    }
    Viewer {
        user_id: id,
        session_digest: digest,
    }
}
fn key_string(key: u32) -> String {
    format!("{key:043}")
}
fn date(seconds: i64) -> String {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
fn create(book_id: i64, slot: PromotionSlot) -> Command {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    Command {
        action: Action::Create,
        campaign_id: None,
        expected_revision: 0,
        details: Some(CampaignSpec {
            book_id,
            slot,
            starts_at: date(now - 60),
            ends_at: date(now + 3600),
            priority: 20,
        }),
    }
}
fn observation(book: i64, r: &Receipt, slot: PromotionSlot) -> CardObservation {
    CardObservation {
        book_id: book,
        surface: slot.surface(),
        editorial: Some(EditorialPlacement {
            campaign_id: r.campaign_id,
            revision: r.revision,
            slot,
        }),
    }
}
async fn totals(pool: &PgPool) -> (i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM reader_wallet_entries),(SELECT count(*) FROM reader_chapter_completions),(SELECT count(*) FROM rec_subjects),(SELECT count(*) FROM outbox_events)").fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn atomic_replays_concurrency_terminal_states_and_no_financial_or_reading_side_effects() {
    let f = Fixture::new().await;
    let service = Editorial::new(&f.store);
    let book = f.book("commands", "basic").await;
    let other_book = f.book("other-target", "basic").await;
    let before = totals(&f.pool).await;
    let command = create(book, PromotionSlot::HomeFeatured);
    let key = key_string(1);
    let (a, b) = tokio::join!(
        service.execute(&f.admin, &key, &command),
        service.execute(&f.admin, &key, &command)
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a.revision, 1);
    assert_eq!(
        f.store.assigned_book(a.campaign_id).await.unwrap(),
        Some(book)
    );
    assert_eq!(f.store.assigned_book(i64::MAX).await.unwrap(), None);
    let mut conflict = command.clone();
    conflict.details.as_mut().unwrap().priority = 70;
    assert_eq!(
        service.execute(&f.admin, &key_string(1), &conflict).await,
        Err(EditorialError::Conflict)
    );
    let active = f.act(&a, Action::Activate, 2).await.unwrap();
    assert!(f
        .store
        .valid_observation(&observation(book, &active, PromotionSlot::HomeFeatured))
        .await
        .unwrap());
    assert!(!f
        .store
        .valid_observation(&observation(book, &a, PromotionSlot::HomeFeatured))
        .await
        .unwrap());
    assert_eq!(
        f.act(&a, Action::Pause, 3).await,
        Err(EditorialError::Conflict)
    );
    let paused = f.act(&active, Action::Pause, 4).await.unwrap();
    assert!(!f
        .store
        .valid_observation(&observation(book, &active, PromotionSlot::HomeFeatured))
        .await
        .unwrap());
    let mut edit = conflict.clone();
    edit.action = Action::Update;
    edit.campaign_id = Some(a.campaign_id);
    edit.expected_revision = paused.revision;
    let mut retarget = edit.clone();
    retarget.details.as_mut().unwrap().book_id = other_book;
    assert_eq!(
        service.execute(&f.admin, &key_string(99), &retarget).await,
        Err(EditorialError::Conflict)
    );
    let mut raw = f.pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('tor.editorial_session',encode($1::bytea,'hex'),true)")
        .bind(f.admin.session_digest.as_slice())
        .execute(&mut *raw)
        .await
        .unwrap();
    let rejected =
        sqlx::query("UPDATE editorial_campaigns SET book_id=$2,revision=revision+1 WHERE id=$1")
            .bind(a.campaign_id)
            .bind(other_book)
            .execute(&mut *raw)
            .await
            .unwrap_err();
    assert_eq!(
        rejected.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    raw.rollback().await.unwrap();
    let changed = service
        .execute(&f.admin, &key_string(5), &edit)
        .await
        .unwrap();
    let resumed = f.act(&changed, Action::Activate, 6).await.unwrap();
    let ended = f.act(&resumed, Action::End, 7).await.unwrap();
    assert_eq!(
        f.store.assigned_book(ended.campaign_id).await.unwrap(),
        Some(book)
    );
    assert_eq!(
        f.act(&ended, Action::Activate, 8).await,
        Err(EditorialError::Conflict)
    );
    // A retry returns the original receipt even though the campaign since moved on.
    assert_eq!(
        service
            .execute(&f.admin, &key_string(1), &command)
            .await
            .unwrap(),
        a
    );
    let audits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM editorial_audit WHERE campaign_id=$1")
            .bind(a.campaign_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(audits, ended.revision);
    for table in ["editorial_commands", "editorial_audit"] {
        let error = sqlx::query(&format!("DELETE FROM {table} WHERE campaign_id=$1"))
            .bind(a.campaign_id)
            .execute(&f.pool)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23514")
        );
    }
    assert_eq!(totals(&f.pool).await, before);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn serving_rechecks_book_age_session_explicit_hide_schedule_and_each_slot() {
    let f = Fixture::new().await;
    let service = Editorial::new(&f.store);
    let basic = f.book("basic", "basic").await;
    let adult = f.book("adult", "adult").await;
    let pro = f.book("pro", "pro_adult").await;
    let host = f.book("host", "basic").await;
    let viewer = user(&f.pool, &format!("editorial-reader-{}", f.stamp), false).await;
    let mut campaigns = Vec::new();
    for (index, slot) in PromotionSlot::ALL.into_iter().enumerate() {
        for (offset, book) in [basic, adult, pro, host].into_iter().enumerate() {
            campaigns.push(
                f.active(book, slot, 100 + (index * 10 + offset * 2) as u32)
                    .await,
            );
        }
        let items = service
            .cards(
                None,
                slot,
                (slot != PromotionSlot::HomeFeatured).then_some(host),
            )
            .await
            .unwrap();
        assert!(items.iter().any(|c| c.book_id == basic));
        assert!(!items.iter().any(|c| c.book_id == adult || c.book_id == pro));
        if slot != PromotionSlot::HomeFeatured {
            assert!(!items.iter().any(|c| c.book_id == host));
        }
    }
    sqlx::query("UPDATE users SET adult_enabled=true WHERE id=$1")
        .bind(viewer.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
    let items = service
        .cards(Some(&viewer), PromotionSlot::HomeFeatured, None)
        .await
        .unwrap();
    assert!(items.iter().any(|c| c.book_id == adult));
    assert!(!items.iter().any(|c| c.book_id == pro));
    sqlx::query("INSERT INTO user_level_grants(user_id,level,source,source_ref_id) VALUES($1,'pro_adult','moderation',NULL)").bind(viewer.user_id).execute(&f.pool).await.unwrap();
    sqlx::query("UPDATE users SET adult_enabled=false WHERE id=$1")
        .bind(viewer.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(
        service
            .cards(Some(&viewer), PromotionSlot::HomeFeatured, None)
            .await
            .unwrap()
            .iter()
            .any(|c| c.book_id == adult),
        "pro_adult also authorizes adult metadata without modifying the opt-in"
    );
    assert!(service
        .cards(Some(&viewer), PromotionSlot::HomeFeatured, None)
        .await
        .unwrap()
        .iter()
        .any(|c| c.book_id == pro));
    sqlx::query("INSERT INTO rec_preference_accounts(user_id) VALUES($1)")
        .bind(viewer.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO rec_book_preferences(user_id,book_id,hidden,revision,expires_at) VALUES($1,$2,true,1,now()+interval '1 day')").bind(viewer.user_id).bind(basic).execute(&f.pool).await.unwrap();
    assert!(!service
        .cards(Some(&viewer), PromotionSlot::HomeFeatured, None)
        .await
        .unwrap()
        .iter()
        .any(|c| c.book_id == basic));
    assert!(service
        .cards(None, PromotionSlot::HomeFeatured, None)
        .await
        .unwrap()
        .iter()
        .any(|c| c.book_id == basic));
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(viewer.session_digest.as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(service
        .cards(Some(&viewer), PromotionSlot::HomeFeatured, None)
        .await
        .unwrap()
        .is_empty());
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(basic)
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(!service
        .cards(None, PromotionSlot::HomeFeatured, None)
        .await
        .unwrap()
        .iter()
        .any(|c| c.book_id == basic));
    assert!(!f
        .store
        .valid_observation(&observation(
            basic,
            &campaigns[0],
            PromotionSlot::HomeFeatured
        ))
        .await
        .unwrap());
    let mut denied = create(basic, PromotionSlot::HomeFeatured);
    assert_eq!(
        service.execute(&f.admin, &key_string(999), &denied).await,
        Err(EditorialError::NotFound)
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    denied.details = Some(CampaignSpec {
        book_id: host,
        slot: PromotionSlot::HomeFeatured,
        starts_at: date(now + 600),
        ends_at: date(now + 1200),
        priority: 100,
    });
    let future = service
        .execute(&f.admin, &key_string(998), &denied)
        .await
        .unwrap();
    let future = f.act(&future, Action::Activate, 997).await.unwrap();
    assert!(!f
        .store
        .valid_observation(&observation(host, &future, PromotionSlot::HomeFeatured))
        .await
        .unwrap());
    sqlx::query("DELETE FROM user_level_grants WHERE user_id=$1")
        .bind(viewer.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(viewer.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn database_rejects_stale_or_forged_administrator_and_bounds_locked_work() {
    let f = Fixture::new().await;
    let book = f.book("authority", "basic").await;
    let draft = f.create(book, PromotionSlot::HomeFeatured, 1).await;
    let before = totals(&f.pool).await;
    let mut fake = f.admin.clone();
    fake.user_id += 1000000;
    assert_eq!(
        Editorial::new(&f.store)
            .execute(
                &fake,
                &key_string(2),
                &create(book, PromotionSlot::HomeFeatured)
            )
            .await,
        Err(EditorialError::Denied)
    );
    // Direct DML has no live A context and cannot bypass the service guard.
    let error = sqlx::query(
        "UPDATE editorial_campaigns SET status='active',revision=revision+1 WHERE id=$1",
    )
    .bind(draft.campaign_id)
    .execute(&f.pool)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let deleted = sqlx::query("DELETE FROM editorial_campaigns WHERE id=$1")
        .bind(draft.campaign_id)
        .execute(&f.pool)
        .await
        .unwrap_err();
    assert_eq!(
        deleted.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=false WHERE user_id=$1 AND role='administrator'",
    )
    .bind(f.admin.user_id)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        f.act(&draft, Action::Activate, 3).await,
        Err(EditorialError::Denied)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=true WHERE user_id=$1 AND role='administrator'",
    )
    .bind(f.admin.user_id)
    .execute(&f.pool)
    .await
    .unwrap();
    let mut locked = f.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM editorial_campaigns WHERE id=$1 FOR UPDATE")
        .bind(draft.campaign_id)
        .execute(&mut *locked)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        f.act(&draft, Action::Activate, 4).await,
        Err(EditorialError::Unavailable)
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    locked.rollback().await.unwrap();
    assert_eq!(
        f.act(&draft, Action::Activate, 4).await.unwrap().revision,
        2,
        "timed-out command left neither receipt nor mutation"
    );
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE token_hash=$1")
        .bind(f.admin.session_digest.as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.act(&draft, Action::Pause, 5).await,
        Err(EditorialError::Denied)
    );
    assert_eq!(totals(&f.pool).await, before);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn campaign_listing_is_bounded_keyset_and_duplicates_never_fill_a_slot() {
    let f = Fixture::new().await;
    let book = f.book("page", "basic").await;
    for key in 1..=52 {
        f.create(book, PromotionSlot::HomeFeatured, key).await;
    }
    let service = Editorial::new(&f.store);
    let first = service.campaigns(None).await.unwrap();
    assert_eq!(first.items.len(), 50);
    let second = service.campaigns(first.next_cursor).await.unwrap();
    assert!(second
        .items
        .iter()
        .all(|c| c.id < first.next_cursor.unwrap()));
    assert!(first.items.windows(2).all(|pair| pair[0].id > pair[1].id));
    f.active(book, PromotionSlot::HomeFeatured, 100).await;
    f.active(book, PromotionSlot::HomeFeatured, 110).await;
    let items = service
        .cards(None, PromotionSlot::HomeFeatured, None)
        .await
        .unwrap();
    assert_eq!(items.iter().filter(|c| c.book_id == book).count(), 1);
    f.clean().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn unavailable_and_duplicate_assignments_do_not_starve_the_next_eligible_book() {
    let f = Fixture::new().await;
    let repeated = f.book("priority-duplicate", "basic").await;
    let next = f.book("priority-next", "basic").await;
    let service = Editorial::new(&f.store);
    for index in 0..128_u32 {
        let mut command = create(repeated, PromotionSlot::HomeFeatured);
        command.details.as_mut().unwrap().priority = 100;
        let draft = service
            .execute(&f.admin, &key_string(1000 + index * 2), &command)
            .await
            .unwrap();
        f.act(&draft, Action::Activate, 1001 + index * 2)
            .await
            .unwrap();
    }
    f.active(next, PromotionSlot::HomeFeatured, 2000).await;
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(repeated)
        .execute(&f.pool)
        .await
        .unwrap();
    let cards = service
        .cards(None, PromotionSlot::HomeFeatured, None)
        .await
        .unwrap();
    assert!(cards.iter().any(|card| card.book_id == next));
    assert!(!cards.iter().any(|card| card.book_id == repeated));
    sqlx::query("UPDATE books SET admin_hold=false WHERE id=$1")
        .bind(repeated)
        .execute(&f.pool)
        .await
        .unwrap();
    let cards = service
        .cards(None, PromotionSlot::HomeFeatured, None)
        .await
        .unwrap();
    assert_eq!(
        cards.iter().filter(|card| card.book_id == repeated).count(),
        1
    );
    assert!(cards.iter().any(|card| card.book_id == next));
    assert!(cards.len() <= 4);
    f.clean().await;
}
