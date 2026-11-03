//! MOD-C03 additional PostgreSQL/HTTP contract checks.
use super::*;

fn last_decision(url: &str, chapter: i64) -> i64 {
    pg(
        url,
        &format!(
            "SELECT COALESCE(MAX(id),0) FROM moderation_chapter_decision_events WHERE chapter_id={chapter}"
        ),
    )
    .parse()
    .unwrap()
}

pub(super) struct Context<'a> {
    pub(super) url: &'a str,
    pub(super) app: &'a Router,
    pub(super) reader: &'a PgReader,
    pub(super) book: i64,
    pub(super) paid_chapter: i64,
    pub(super) paid_version: i64,
    pub(super) paid_action: &'a str,
    pub(super) mod_cookie: &'a str,
    pub(super) admin_cookie: &'a str,
    pub(super) csrf: &'a str,
}

pub(super) async fn check(context: Context<'_>) {
    let Context {
        url,
        app,
        reader,
        book,
        paid_chapter,
        paid_version,
        paid_action,
        mod_cookie,
        admin_cookie,
        csrf,
    } = context;
    // MOD-C03: publish a first FREE working edition atomically, then draft
    // and republish exactly that edition. Never synthesize a paid publication.
    let pending_free: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
             VALUES({book},4,'Moderator first release','draft','free',0,1) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let pending_version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({pending_free},1,'MOD-C03 first released immutable body') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_working_drafts(chapter_id,version_id)
             VALUES({pending_free},{pending_version})"
        ),
    );
    let pending_action = format!("/moderation/chapters/{pending_free}/hold");
    let publish_pending = format!(
        "csrf_token={csrf}&expected_state=none&next_state=published&expected_visibility=draft&expected_revision=1&expected_version={pending_version}&expected_decision_id=0"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &pending_action,
            &mod_cookie,
            ORIGIN,
            &publish_pending
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
        "revoked moderator role must also deny publish"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &pending_action,
            &admin_cookie,
            "https://invalid.example",
            &publish_pending
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &pending_action,
            &admin_cookie,
            ORIGIN,
            &publish_pending
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT c.visibility || ':' || c.admin_hold::int || ':' ||
                        p.version_id || ':' ||
                        (SELECT COUNT(*) FROM chapter_first_release_events e WHERE e.chapter_id=c.id)
                 FROM chapters c
                 JOIN chapter_publications p ON p.chapter_id=c.id
                 WHERE c.id={pending_free}"
            )
        ),
        format!("public:0:{pending_version}:1")
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapter_working_drafts WHERE chapter_id={pending_free}")
        ),
        "0"
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/reader/{pending_free}"),
            "",
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &pending_action,
            &admin_cookie,
            ORIGIN,
            &publish_pending
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let first_decision_id = last_decision(&url, pending_free);
    let pending_draft = format!(
        "csrf_token={csrf}&expected_state=none&next_state=draft&expected_visibility=public&expected_revision=1&expected_version={pending_version}&expected_decision_id={first_decision_id}"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &pending_action,
            &admin_cookie,
            ORIGIN,
            &pending_draft
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/reader/{pending_free}"),
            "",
            ORIGIN,
            ""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT visibility || ':' || moderation_state FROM chapters WHERE id={pending_free}")
        ),
        "draft:none"
    );
    // The first publish form is stale even after visibility returns to draft.
    assert_eq!(
        request(
            &app,
            "POST",
            &pending_action,
            &admin_cookie,
            ORIGIN,
            &publish_pending
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let second_decision_id = last_decision(&url, pending_free);
    let pending_republish = format!(
        "csrf_token={csrf}&expected_state=none&next_state=published&expected_visibility=draft&expected_revision=1&expected_version={pending_version}&expected_decision_id={second_decision_id}"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &pending_action,
            &admin_cookie,
            ORIGIN,
            &pending_republish
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id={pending_free}"
            )
        ),
        "1",
        "republish must never create a second first-release fact"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT before_state || ':' || after_state
                 FROM moderation_chapter_decision_events
                 WHERE chapter_id={pending_free} ORDER BY id"
            )
        ),
        "draft:published\npublished:draft\ndraft:published"
    );

    // MOD-C03 paid draft/republication preserves exactly the existing
    // B-owned pointer, price, and all historical entitlements.
    let paid_decision_id = last_decision(&url, paid_chapter);
    let paid_to_draft = format!(
        "csrf_token={csrf}&expected_state=none&next_state=draft&expected_visibility=public&expected_revision=0&expected_version={paid_version}&expected_decision_id={paid_decision_id}"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &paid_to_draft
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(reader
        .public_paid_chapter_preview(paid_chapter as u64)
        .await
        .unwrap()
        .is_none());
    let paid_draft_decision_id = last_decision(&url, paid_chapter);
    let paid_to_public = format!(
        "csrf_token={csrf}&expected_state=none&next_state=published&expected_visibility=draft&expected_revision=0&expected_version={paid_version}&expected_decision_id={paid_draft_decision_id}"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &paid_action,
            &admin_cookie,
            ORIGIN,
            &paid_to_public
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT c.visibility || ':' || c.price_amount || ':' || p.version_id
                 FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
                 WHERE c.id={paid_chapter}"
            )
        ),
        format!("public:199:{paid_version}")
    );
    assert!(reader
        .public_paid_chapter_preview(paid_chapter as u64)
        .await
        .unwrap()
        .is_some());

    // A version with no B publication pointer is not safe to expose as paid.
    let unverified_paid: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
             VALUES({book},5,'Unverified paid draft','draft','paid',150,1) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let unverified_version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({unverified_paid},1,'Unverified private edition') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_working_drafts(chapter_id,version_id)
             VALUES({unverified_paid},{unverified_version})"
        ),
    );
    let unverified_form = format!(
        "csrf_token={csrf}&expected_state=none&next_state=published&expected_visibility=draft&expected_revision=1&expected_version={unverified_version}&expected_decision_id=0"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/moderation/chapters/{unverified_paid}/hold"),
            &admin_cookie,
            ORIGIN,
            &unverified_form
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert!(reader
        .public_paid_chapter_preview(unverified_paid as u64)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_publications WHERE chapter_id={unverified_paid}"
            )
        ),
        "0"
    );

    // No current edition: publishing a draft must fail without an audit row.
    let empty_chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},6,'No current version','draft','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            &format!("/moderation/chapters/{empty_chapter}/hold"),
            &admin_cookie,
            ORIGIN,
            &format!(
                "csrf_token={csrf}&expected_state=none&next_state=published&expected_visibility=draft&expected_revision=0&expected_decision_id=0"
            )
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM moderation_chapter_decision_events
                 WHERE chapter_id={empty_chapter}"
            )
        ),
        "0"
    );
}
