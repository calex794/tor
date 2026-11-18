//! RET-R03a: one verified demand -> owner editorial choice -> actual chapter release
//! -> the same reader's completed return. Status is Book-level, NOT a durable
//! per-frontier editorial decision (the next R03 slice must own that linkage).
use axum::{http::StatusCode, Router};

pub(super) async fn verify_owner_choice(
    app: &Router,
    url: &str,
    book: i64,
    owner_cookie: &str,
    reader_cookie: &str,
    csrf: &str,
) {
    let author_path = format!("/me/books/{book}");
    let status_path = format!("/me/books/{book}/settings/project-status");
    let owner_page =
        super::html(super::call(app, "GET", &author_path, owner_cookie, super::ORIGIN, "").await)
            .await;
    assert!(
        owner_page.contains(&format!("href=\"{status_path}\"")),
        "qualified demand must lead to the existing editorial state editor"
    );
    assert!(
        owner_page.contains("Запросов после последней опубликованной главы: <strong>1</strong>")
    );
    assert!(
        !owner_page.contains("c15-reader-"),
        "editorial demand is bounded and aggregated, never a reader list"
    );
    let unauthorized =
        super::call(app, "GET", &status_path, reader_cookie, super::ORIGIN, "").await;
    assert_eq!(
        unauthorized.status(),
        StatusCode::NOT_FOUND,
        "a reader cannot view the owner's editorial state"
    );
    let editable = super::call(app, "GET", &status_path, owner_cookie, super::ORIGIN, "").await;
    assert_eq!(editable.status(), StatusCode::OK);
    assert!(
        super::html(editable).await.contains("Состояние работы"),
        "the decision is a real owner form, not a fictitious approval"
    );
    let revision: i64 = super::pg(
        url,
        &format!("SELECT project_status_revision FROM books WHERE id={book}"),
    )
    .parse()
    .unwrap();
    assert!(revision > 0);
    let form = format!("project_status=ongoing&expected_revision={revision}&csrf_token={csrf}");
    assert_eq!(
        super::call(
            app,
            "POST",
            &status_path,
            reader_cookie,
            super::ORIGIN,
            &form,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "a reader cannot make an editorial decision"
    );
    assert_eq!(
        super::call(
            app,
            "POST",
            &status_path,
            owner_cookie,
            super::ORIGIN,
            &form,
        )
        .await
        .status(),
        StatusCode::SEE_OTHER,
        "the owner explicitly decides to work on the requested continuation"
    );
    let stored = super::pg(
        url,
        &format!(
            "SELECT project_status || ':' || project_status_revision
             FROM books WHERE id={book}"
        ),
    );
    assert_eq!(stored, format!("ongoing:{}", revision + 1));
    assert_eq!(
        super::call(
            app,
            "POST",
            &status_path,
            owner_cookie,
            super::ORIGIN,
            &form,
        )
        .await
        .status(),
        StatusCode::CONFLICT,
        "an old editor tab cannot overwrite the editorial decision"
    );
    assert_eq!(
        super::pg(
            url,
            &format!("SELECT count(*) FROM chapters WHERE book_id={book} AND chapter_no>1")
        ),
        "0",
        "a status decision must never silently publish text"
    );
    assert_eq!(
        super::pg(
            url,
            &format!("SELECT count(*) FROM reader_continuation_demand WHERE book_id={book}")
        ),
        "1",
        "changing editorial state does not erase the current demand cycle"
    );
}

pub(super) fn verify_release_outcome(
    url: &str,
    book: i64,
    old_frontier: i64,
    released: i64,
    reader: i64,
) {
    // A genuinely published next chapter and the returning reader's durable
    // completed text are required. A visit, Follow, queued event, or project
    // status alone cannot satisfy this evidence check.
    assert_eq!(
        super::pg(
            url,
            &format!(
                "SELECT count(*) FROM reader_continuation_demand AS demand
                 JOIN chapters AS next ON next.book_id=demand.book_id
                 JOIN chapter_first_release_events AS release ON release.chapter_id=next.id
                 JOIN reader_chapter_completions AS finished
                   ON finished.user_id=demand.user_id AND finished.chapter_id=next.id
                 JOIN books AS book ON book.id=demand.book_id
                 WHERE demand.user_id={reader}
                   AND demand.book_id={book}
                   AND demand.frontier_chapter_id={old_frontier}
                   AND next.id={released}
                   AND next.chapter_no=(SELECT chapter_no+1 FROM chapters
                                        WHERE id={old_frontier})
                   AND book.project_status='ongoing'"
            )
        ),
        "1",
        "R03 requires real demand, an owner choice, canonical release, and verified return"
    );
}
