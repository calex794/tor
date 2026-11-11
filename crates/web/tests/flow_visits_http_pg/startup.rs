//! Exercise the production entry point: turning off AUTH must not stop erasure.
use super::pg;
use std::{
    collections::BTreeSet,
    net::TcpListener,
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tor_books::repositories::PgCatalog;
#[path = "startup_funnel.rs"]
mod funnel;
#[path = "startup_preferences.rs"]
mod preferences;
#[path = "startup_retirement.rs"]
mod retirement;

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn retained(url: &str, actor: i64) -> String {
    pg(
        url,
        &format!(
            "SELECT concat_ws('|',
               (SELECT count(*) FROM flow_visits WHERE subject_id=s.subject_id),
               (SELECT count(*) FROM flow_visit_receipts WHERE subject_id=s.subject_id),
               (SELECT count(*) FROM flow_visit_account_days WHERE subject_id=s.subject_id),
               (SELECT count(*) FROM flow_visit_continuations WHERE subject_id=s.subject_id))
             FROM rec_subjects s WHERE user_id={actor}"
        ),
    )
}

// UTC account-day rows describe dates, not a fixed number of recent visits.
// Between 00:10 and 00:40 UTC the two live visits straddle midnight.
fn account_day_counts(anchor_seconds: i64) -> (usize, usize) {
    let recent: BTreeSet<_> = [40 * 60, 10 * 60]
        .into_iter()
        .map(|age| (anchor_seconds - age).div_euclid(86_400))
        .collect();
    let mut all = recent.clone();
    all.insert((anchor_seconds - 31 * 86_400).div_euclid(86_400));
    (all.len(), recent.len())
}

#[test]
fn account_day_fixture_covers_both_sides_of_utc_midnight() {
    for (seconds, expected) in [
        (0, (2, 1)),
        (599, (2, 1)),
        (600, (3, 2)),
        (2_399, (3, 2)),
        (2_400, (2, 1)),
        (86_399, (2, 1)),
    ] {
        assert_eq!(account_day_counts(seconds), expected);
        assert_eq!(account_day_counts(20_000 * 86_400 + seconds), expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn public_postgres_startup_retires_legacy_flags_and_preserves_cleanup() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = pg(
        &url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('flow-startup-{stamp}@tor-ci.example','FLOW startup','$argon2id$test')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    // Capture one database clock for both fixture timestamps and expected UTC dates.
    let anchor: i64 = pg(&url, "SELECT floor(extract(epoch FROM now()))::bigint")
        .parse()
        .unwrap();
    let (seeded_days, retained_days) = account_day_counts(anchor);
    let expected_retained = format!("2|2|{retained_days}|0");
    // Backdated rows model an already opted-in installation after a restart.
    // The test itself never calls maintain(), starts a helper worker or offers
    // a fresh visit: only the real binary may expire this data.
    pg(
        &url,
        &format!(
            "INSERT INTO rec_subjects(user_id,enabled,enabled_at,flow_epoch,flow_enabled_at)
             VALUES({actor},true,now()-interval '40 days',gen_random_uuid(),now()-interval '40 days');
             WITH seeded AS MATERIALIZED (
               SELECT s.subject_id,s.flow_epoch,gen_random_uuid() AS id,to_timestamp({anchor})-age AS at
               FROM rec_subjects s CROSS JOIN (
                 VALUES(interval '31 days'),(interval '40 minutes'),(interval '10 minutes')
               ) AS ages(age) WHERE s.user_id={actor}
             ) INSERT INTO flow_visits(id,subject_id,flow_epoch,started_at,last_at,
                 first_event_id,last_event_id,landing_surface,last_surface,acquisition_hint,pageviews)
               SELECT id,subject_id,flow_epoch,at,at,id,id,4,4,1,1 FROM seeded;
             INSERT INTO flow_visit_receipts(event_id,subject_id,flow_epoch,observed_at,surface,acquisition_hint)
               SELECT id,v.subject_id,v.flow_epoch,started_at,4,1 FROM flow_visits v
               JOIN rec_subjects s ON s.subject_id=v.subject_id WHERE s.user_id={actor};
             INSERT INTO flow_visit_account_days(subject_id,flow_epoch,bucket_date)
               SELECT DISTINCT v.subject_id,v.flow_epoch,(started_at AT TIME ZONE 'UTC')::date
               FROM flow_visits v JOIN rec_subjects s ON s.subject_id=v.subject_id WHERE s.user_id={actor};
             INSERT INTO flow_visit_continuations(subject_id,flow_epoch,last_at)
               SELECT subject_id,flow_epoch,now()-interval '40 minutes' FROM rec_subjects WHERE user_id={actor}"
        ),
    );
    assert_eq!(retained(&url, actor), format!("3|3|{seeded_days}|1"));
    let expired_book = funnel::seed(&url, actor).await;
    let preference_probe = preferences::seed(&url, actor, expired_book).await;
    retirement::seed(&url, actor, expired_book);
    let rollup = format!("SELECT COALESCE(sum(visits),0) FROM flow_visit_daily
                  WHERE cohort_day=((to_timestamp({anchor})-interval '31 days') AT TIME ZONE 'UTC')::date
                    AND acquisition_hint=1 AND landing_surface=4 AND last_surface=4");
    let before_rollup: i64 = pg(&url, &rollup).parse().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_tor-web"))
            .env_clear()
            .env("DATABASE_URL", &url)
            .env("TOR_LISTEN_ADDR", address.to_string())
            .env("TOR_DEMO_MODE", "0")
            .env("TOR_AUTH_ENABLED", "0")
            .env("TOR_RECOMMENDATIONS_ENABLED", "0")
            .env("TOR_FLOW_VISITS_ENABLED", "1")
            .env("TOR_REC_ATTRIBUTION_ENABLED", "1")
            .env("TOR_ORGANIC_ATTRIBUTION_ENABLED", "1")
            .env("TOR_READING_RETURNS_ENABLED", "1")
            .env("TOR_BOOK_READING_RETURNS_ENABLED", "1")
            .env("TOR_FLOW_PUBLIC_OPT_IN_ENABLED", "1")
            .env("TOR_RIL_SINGLE_INSTANCE", "1")
            .env("TOR_RIL_HMAC_KEY", "11".repeat(32))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("production tor-web binary"),
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(750))
        .build()
        .unwrap();
    let origin = format!("http://{address}");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(server.0.try_wait().unwrap().is_none(), "server exited");
        if client
            .get(format!("{origin}/readyz"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
            && retained(&url, actor) == expected_retained
            && funnel::expired(&url, actor)
            && preferences::expired(&url, actor, &preference_probe)
            && retirement::expired(&url, actor)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "startup did not expire FLOW data"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(pg(&url, &rollup).parse::<i64>().unwrap(), before_rollup + 1);
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FILTER(WHERE v.finalized_at IS NOT NULL)
             FROM flow_visits v JOIN rec_subjects s ON s.subject_id=v.subject_id
             WHERE s.user_id={actor}"
            )
        ),
        "1",
        "idle unexpired visit was finalized, fresh activity stayed mutable"
    );
    for _ in 0..3 {
        assert!(client
            .get(format!("{origin}/"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }
    assert_eq!(retained(&url, actor), expected_retained);
    funnel::assert_preserved(&url, actor, expired_book);
    preferences::assert_preserved(&url, actor, &preference_probe);
    retirement::assert_preserved(&url, actor, expired_book);
    retirement::assert_no_new_consumption(&url, actor, expired_book).await;
    drop(server);
    preferences::cleanup(&url, &preference_probe);
    pg(&url, &format!("DELETE FROM users WHERE id={actor}"));
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={expired_book}"),
    );
}
