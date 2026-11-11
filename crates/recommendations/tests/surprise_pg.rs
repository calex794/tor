//! Real PostgreSQL acceptance on disposable tor_ci only. No production writes.
use sqlx::{postgres::PgPoolOptions, PgPool};
use tor_recommendations::repositories::surprises::{PgSurprises, PlanState, SurpriseError};

struct Fixture {
    app: PgPool,
    owner: PgPool,
    schema: String,
    actor: i64,
    store: PgSurprises,
}

impl Fixture {
    async fn new(label: &str) -> Self {
        let app_url = std::env::var("TEST_DATABASE_URL").expect("isolated app URL");
        let owner_url = std::env::var("MIGRATOR_URL").expect("isolated migrator URL");
        assert!(app_url.ends_with("/tor_ci") && owner_url.ends_with("/tor_ci"));
        let owner = PgPool::connect(&owner_url).await.unwrap();
        let schema = format!("surprise_{label}");
        sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&owner)
            .await
            .unwrap();
        for table in [
            "surprise_runtime_policy",
            "surprise_content",
            "surprise_user_preferences",
            "surprise_daily_decisions",
            "surprise_admin_events",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
        sqlx::query(&format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"))
            .execute(&owner)
            .await
            .unwrap();
        sqlx::query(&format!(
            "GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO tor_app"
        ))
        .execute(&owner)
        .await
        .unwrap();
        sqlx::query(&format!(
            "GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA {schema} TO tor_app"
        ))
        .execute(&owner)
        .await
        .unwrap();
        let path = schema.clone();
        let app = PgPoolOptions::new()
            .max_connections(5)
            .after_connect(move |connection, _| {
                let cmd = format!("SET search_path={path},public");
                Box::pin(async move {
                    sqlx::query(&cmd).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        sqlx::query("INSERT INTO surprise_runtime_policy(singleton) VALUES(true)")
            .execute(&app)
            .await
            .unwrap();
        let email = format!("surprise-{label}@tor-ci.example");
        sqlx::query("DELETE FROM users WHERE email_normalized=$1")
            .bind(&email)
            .execute(&app)
            .await
            .unwrap();
        let actor: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'Surprise test','$argon2id$test') RETURNING id",
        )
        .bind(&email)
        .fetch_one(&app)
        .await
        .unwrap();
        let store = PgSurprises::new(app.clone());
        Self {
            app,
            owner,
            schema,
            actor,
            store,
        }
    }

    async fn enable(&self) {
        sqlx::query("UPDATE surprise_runtime_policy SET enabled=true,probability_per_mille=1000")
            .execute(&self.app)
            .await
            .unwrap();
    }

    async fn finish(self) {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.actor)
            .execute(&self.app)
            .await
            .unwrap();
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn surprise_plan_is_default_off_replay_safe_and_capped_between_tabs() {
    let f = Fixture::new("plans").await;
    let key = [0x7d; 32];
    assert_eq!(f.store.plan(f.actor, &key).await.unwrap(), PlanState::Off);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM surprise_daily_decisions")
        .fetch_one(&f.app)
        .await
        .unwrap();
    assert_eq!(count, 0, "feature OFF cannot write decision history");
    f.enable().await;
    sqlx::query(
        "INSERT INTO surprise_content(kind,title,body,state)
         VALUES('joke','Сюрприз','Редакционная шутка','approved')",
    )
    .execute(&f.app)
    .await
    .unwrap();
    let plan = f.store.plan(f.actor, &key).await.unwrap();
    let PlanState::Planned {
        utc_day,
        active_after_seconds,
    } = plan
    else {
        panic!("100% probability must plan");
    };
    assert!((120..=1200).contains(&active_after_seconds));
    let another_device = f.store.clone();
    assert_eq!(another_device.plan(f.actor, &key).await.unwrap(), plan);
    sqlx::query(
        "UPDATE surprise_daily_decisions SET created_at=now()-interval '30 minutes'
         WHERE user_id=$1",
    )
    .bind(f.actor)
    .execute(&f.app)
    .await
    .unwrap();
    let (first, second) = tokio::join!(
        f.store.claim(f.actor, utc_day, 1200),
        another_device.claim(f.actor, utc_day, 1200)
    );
    assert_eq!(first.unwrap() as u8 + second.unwrap() as u8, 1);
    assert!(!f.store.claim(f.actor, utc_day, 1200).await.unwrap());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn surprise_open_rechecks_public_basic_book_and_disable() {
    let f = Fixture::new("visibility").await;
    f.enable().await;
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES($1,'ru','Surprise public book','public','basic') RETURNING id",
    )
    .bind("ci:surprise:visibility")
    .fetch_one(&f.app)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO surprise_content(kind,title,body,book_id,editorial,state)
         VALUES('book','Тестовая рекомендация','Новая книга', $1,true,'approved')",
    )
    .bind(book)
    .execute(&f.app)
    .await
    .unwrap();
    let key = [0xaf; 32];
    let plan = f.store.plan(f.actor, &key).await.unwrap();
    let PlanState::Planned { utc_day, .. } = plan else {
        panic!("selected plan required");
    };
    sqlx::query(
        "UPDATE surprise_daily_decisions SET created_at=now()-interval '30 minutes'
         WHERE user_id=$1",
    )
    .bind(f.actor)
    .execute(&f.app)
    .await
    .unwrap();
    assert!(f.store.claim(f.actor, utc_day, 1200).await.unwrap());
    let item = f.store.open(f.actor, utc_day).await.unwrap().unwrap();
    assert_eq!(item.book_id, Some(book));
    assert!(item.editorial);
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(book)
        .execute(&f.app)
        .await
        .unwrap();
    assert!(f.store.open(f.actor, utc_day).await.unwrap().is_none());
    f.store.disable(f.actor).await.unwrap();
    assert!(f.store.open(f.actor, utc_day).await.unwrap().is_none());
    assert!(matches!(
        f.store.plan(f.actor, &key).await,
        Ok(PlanState::Off)
    ));
    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&f.app)
        .await
        .unwrap();
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires isolated disposable tor_ci PostgreSQL"]
async fn surprise_admin_requires_live_role_and_audits_bounded_editorial_changes() {
    use tor_recommendations::repositories::surprises::admin::NewSurprise;

    let f = Fixture::new("admin").await;
    let digest = [0xb7_u8; 32];
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,now()+interval '1 hour')",
    )
    .bind(digest.as_slice())
    .bind(f.actor)
    .execute(&f.app)
    .await
    .unwrap();

    let input = NewSurprise {
        kind: "joke",
        title: "Редакционная находка",
        body: "Короткая шутка без спойлеров.",
        book_id: None,
        editorial: false,
    };
    assert!(matches!(
        f.store.admin_create(&digest, &input).await,
        Err(SurpriseError::Forbidden)
    ));
    assert!(matches!(
        f.store.admin_policy(&digest, true, 350).await,
        Err(SurpriseError::Forbidden)
    ));

    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',true)",
    )
    .bind(f.actor)
    .execute(&f.app)
    .await
    .unwrap();

    let id = f.store.admin_create(&digest, &input).await.unwrap();
    assert!(id > 0);
    assert!(f.store.admin_state(&digest, id, "approved").await.unwrap());
    assert!(!f.store.admin_state(&digest, id, "approved").await.unwrap());
    f.store.admin_policy(&digest, true, 350).await.unwrap();
    let (enabled, chance, items) = f.store.admin_overview(&digest).await.unwrap();
    assert!(enabled);
    assert_eq!(chance, 350);
    assert!(items.iter().any(|row| row.id == id && row.state == "approved"));

    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM surprise_admin_events WHERE actor_user_id=$1",
    )
    .bind(f.actor)
    .fetch_one(&f.app)
    .await
    .unwrap();
    assert_eq!(events, 3, "one creation, approval and policy change");

    sqlx::query(
        "UPDATE account_roles SET is_active=false
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(f.actor)
    .execute(&f.app)
    .await
    .unwrap();
    assert!(matches!(
        f.store.admin_state(&digest, id, "paused").await,
        Err(SurpriseError::Forbidden)
    ));
    assert!(matches!(
        f.store.admin_overview(&digest).await,
        Err(SurpriseError::Forbidden)
    ));
    f.finish().await;
}
