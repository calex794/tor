//! Preference expiry uses the real AUTH=0 startup, without the REC worker.
use super::pg;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_books::repositories::PgCatalog;
use tor_recommendations::{domain::feedback::Choice, repositories::PgRecommendations};

pub(super) struct Probe {
    live_book: i64,
    live_state: String,
    live_command: String,
    epoch_before: i64,
}

fn state(url: &str, actor: i64, book: i64) -> String {
    pg(
        url,
        &format!(
            "SELECT json_build_array(hidden,revision,expires_at::text)
             FROM rec_book_preferences WHERE user_id={actor} AND book_id={book}"
        ),
    )
}

fn live_command(url: &str, actor: i64) -> String {
    pg(
        url,
        &format!(
            "SELECT json_build_array(encode(command_key,'hex'),encode(payload_hash,'hex'),
               result_revision,command_type,created_at::text)
             FROM rec_user_preference_commands
             WHERE user_id={actor} AND created_at>now()-interval '1 day'"
        ),
    )
}

pub(super) async fn seed(url: &str, actor: i64, expired_book: i64) -> Probe {
    let live_book: i64 = pg(
        url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:preference-startup:{actor}','ru','Unexpired explicit choice',
               'public','basic') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let catalog = PgCatalog::connect(url).await.unwrap();
    let preferences = PgRecommendations::new(catalog.pool().clone());
    let expired_key = generate_session_token();
    preferences
        .set_feedback(actor, expired_book, Choice::NotInterested, 0, &expired_key)
        .await
        .unwrap();
    preferences
        .set_feedback(
            actor,
            live_book,
            Choice::NotInterested,
            0,
            &generate_session_token(),
        )
        .await
        .unwrap();
    // Age only the first real command and its explicit preference, after all
    // commands are committed. No fixture helper performs expiry after this.
    let expired_digest: String = session_token_digest(&expired_key)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    pg(
        url,
        &format!(
            "UPDATE rec_book_preferences SET expires_at=now()-interval '2 hours'
             WHERE user_id={actor} AND book_id={expired_book};
             UPDATE rec_user_preference_commands SET created_at=now()-interval '31 days'
             WHERE user_id={actor} AND command_key=decode('{expired_digest}','hex')"
        ),
    );
    catalog.pool().close().await;
    let probe = Probe {
        live_book,
        live_state: state(url, actor, live_book),
        live_command: live_command(url, actor),
        epoch_before: pg(
            url,
            &format!("SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id={actor}"),
        )
        .parse()
        .unwrap(),
    };
    assert_eq!(
        pg(
            url,
            &format!("SELECT count(*) FROM rec_book_preferences WHERE user_id={actor}")
        ),
        "2"
    );
    assert!(!expired(url, actor, &probe));
    probe
}

pub(super) fn expired(url: &str, actor: i64, probe: &Probe) -> bool {
    pg(
        url,
        &format!(
            "SELECT concat_ws('|',
               (SELECT count(*) FROM rec_book_preferences WHERE user_id={actor}),
               (SELECT count(*) FROM rec_user_preference_commands WHERE user_id={actor}),
               (SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id={actor}))"
        ),
    ) == format!("1|1|{}", probe.epoch_before + 1)
}

pub(super) fn assert_preserved(url: &str, actor: i64, probe: &Probe) {
    assert!(expired(url, actor, probe));
    assert_eq!(state(url, actor, probe.live_book), probe.live_state);
    assert_eq!(live_command(url, actor), probe.live_command);
}

pub(super) fn cleanup(url: &str, probe: &Probe) {
    pg(
        url,
        &format!(
            "UPDATE books SET admin_hold=true WHERE id={}",
            probe.live_book
        ),
    );
}

pub(super) async fn arm_next_pass(url: &str, actor: i64, book: i64, probe: &Probe) {
    let catalog = PgCatalog::connect(url).await.unwrap();
    let key = generate_session_token();
    PgRecommendations::new(catalog.pool().clone())
        .set_feedback(
            actor,
            book,
            Choice::NotInterested,
            (probe.epoch_before + 1) * 1_000_000_000,
            &key,
        )
        .await
        .unwrap();
    catalog.pool().close().await;
    let digest: String = session_token_digest(&key)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    pg(
        url,
        &format!(
            "UPDATE rec_book_preferences SET expires_at=now()-interval '2 hours'
           WHERE user_id={actor} AND book_id={book};
         UPDATE rec_user_preference_commands SET created_at=now()-interval '31 days'
           WHERE user_id={actor} AND command_key=decode('{digest}','hex')"
        ),
    );
}

pub(super) fn next_pass_done(url: &str, actor: i64, book: i64, probe: &Probe) -> bool {
    pg(url, &format!(
        "SELECT concat_ws('|',(SELECT count(*) FROM rec_book_preferences WHERE user_id={actor} AND book_id={book}),
          (SELECT feedback_epoch FROM rec_preference_accounts WHERE user_id={actor}))"
    )) == format!("0|{}", probe.epoch_before + 2)
}

pub(super) fn assert_live_unchanged(url: &str, actor: i64, probe: &Probe) {
    assert_eq!(state(url, actor, probe.live_book), probe.live_state);
    assert_eq!(live_command(url, actor), probe.live_command);
}
