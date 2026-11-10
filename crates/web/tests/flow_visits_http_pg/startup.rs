//! Exercise the production entry point: turning off AUTH must not stop erasure.
use super::*;
use std::{
    net::TcpListener,
    process::{Child, Stdio},
    time::Instant,
};
#[path = "startup_funnel.rs"]
mod funnel;
#[path = "startup_preferences.rs"]
mod preferences;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn public_postgres_startup_erases_expired_flow_without_auth_or_collection() {
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
    // Backdated rows model an already opted-in installation after a restart.
    // The test itself never calls maintain(), starts a helper worker or offers
    // a fresh visit: only the real binary may expire this data.
    pg(
        &url,
        &format!(
            "INSERT INTO rec_subjects(user_id,enabled,enabled_at,flow_epoch,flow_enabled_at)
             VALUES({actor},true,now()-interval '40 days',gen_random_uuid(),now()-interval '40 days');
             WITH seeded AS MATERIALIZED (
               SELECT s.subject_id,s.flow_epoch,gen_random_uuid() AS id,now()-age AS at
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
    assert_eq!(retained(&url, actor), "3|3|2|1");
    let expired_book = funnel::seed(&url, actor).await;
    let preference_probe = preferences::seed(&url, actor, expired_book).await;
    let rollup = "SELECT COALESCE(sum(visits),0) FROM flow_visit_daily
                  WHERE cohort_day=((now()-interval '31 days') AT TIME ZONE 'UTC')::date
                    AND acquisition_hint=1 AND landing_surface=4 AND last_surface=4";
    let before_rollup: i64 = pg(&url, rollup).parse().unwrap();
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
            .env("TOR_FLOW_VISITS_ENABLED", "0")
            .env("TOR_RECOMMENDATIONS_ENABLED", "0")
            .env("TOR_REC_ATTRIBUTION_ENABLED", "0")
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
            && retained(&url, actor) == "2|2|1|0"
            && funnel::expired(&url, actor)
            && preferences::expired(&url, actor, &preference_probe)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "startup did not expire FLOW data"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(pg(&url, rollup).parse::<i64>().unwrap(), before_rollup + 1);
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
    assert_eq!(retained(&url, actor), "2|2|1|0");
    funnel::assert_preserved(&url, actor, expired_book);
    preferences::assert_preserved(&url, actor, &preference_probe);
    drop(server);
    preferences::cleanup(&url, &preference_probe);
    pg(&url, &format!("DELETE FROM users WHERE id={actor}"));
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={expired_book}"),
    );
}
