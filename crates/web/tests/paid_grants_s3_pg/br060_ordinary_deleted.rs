//! BR-060: free V1, purchased V1 and ordinary-deletion archive over real HTTP.
//! The caller sends the fresh paid grant to the actual MinIO/WebCrypto consumer.
use super::*;

async fn save_free_v1_before_removal(
    app: &Router,
    url: &str,
    book: i64,
    free_chapter: i64,
    buyer_cookie: &str,
    csrf: &str,
) {
    // This is the same CSRF-protected action a signed-in reader uses to
    // bookmark a public Book and atomically pin all released free versions.
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/me/library/{book}"))
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(
            header::COOKIE,
            format!("tor-dev-session={buyer_cookie}; tor-dev-csrf={csrf}"),
        )
        .body(Body::from(format!("csrf_token={csrf}")))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            41003,
        ))));
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );

    let original_v1 = pg(
        url,
        &format!(
            "SELECT v.version_no FROM reader_saved_free_chapters pin \
             JOIN chapter_versions v ON v.id=pin.version_id \
             JOIN chapters c ON c.id=pin.chapter_id \
             JOIN users u ON u.id=pin.user_id \
             JOIN sessions s ON s.user_id=u.id \
             WHERE c.id={free_chapter} AND c.book_id={book} \
               AND s.revoked_at IS NULL AND s.expires_at>now() \
             LIMIT 1"
        ),
    );
    assert_eq!(original_v1, "1");

    // The author's later free V2 must not replace the reader's saved V1.
    let v2 = scalar(
        url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) \
             VALUES({free_chapter},2,'UNPINNED FREE V2') RETURNING id"
        ),
    );
    pg(
        url,
        &format!(
            "UPDATE chapter_publications SET version_id={v2} \
             WHERE chapter_id={free_chapter}"
        ),
    );
}

/// A true second A login after ordinary deletion: the first session cookie
/// is not forwarded to the grant or archive reader. No test-only actor header.
struct FreshBuyerLogin {
    session: String,
    csrf: String,
}

fn login_cookie(response: &Response, name: &str) -> String {
    let prefix = format!("{name}=");
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .find_map(|cookie| {
            cookie
                .strip_prefix(&prefix)
                .and_then(|value| value.split(';').next())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("missing {name} Set-Cookie"))
}

async fn fresh_buyer_login(app: &Router) -> FreshBuyerLogin {
    let login = private_get(app, "/login", None).await;
    assert_eq!(login.status(), StatusCode::OK);
    let csrf_cookie = login_cookie(&login, "tor-dev-csrf");
    let page = html_body(login).await;
    let form_csrf = page
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("real login must provide CSRF")
        .split('"')
        .next()
        .expect("login CSRF value");
    let email_form = BR060_BUYER_EMAIL.replace('@', "%40");
    let body = format!(
        "email={email_form}&password=S034RealStorageE2EPassword2026&csrf_token={form_csrf}"
    );
    let mut request = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("tor-dev-csrf={csrf_cookie}"))
        .body(Body::from(body))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            41009,
        ))));
    let login_response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(login_response.status(), StatusCode::SEE_OTHER);
    FreshBuyerLogin {
        session: login_cookie(&login_response, "tor-dev-session"),
        csrf: csrf_cookie,
    }
}

/// Return a grant issued *after* removal: a stale URL or SQL purchase is not
/// sufficient for the downstream real S3/WebCrypto acceptance.
pub(super) async fn verify(
    app: &Router,
    url: &str,
    book: i64,
    purchased: (i64, i64, i64),
    chapters: (i64, i64, i64),
    sessions: (&str, &str),
    csrf: &str,
) -> Value {
    let (chapter, purchase_id, v1) = purchased;
    let (free_chapter, locked_chapter, draft_chapter) = chapters;
    let (buyer_cookie, stranger_cookie) = sessions;
    save_free_v1_before_removal(app, url, book, free_chapter, buyer_cookie, csrf).await;
    pg(
        url,
        &format!(
            "UPDATE books SET lifecycle_status='ordinary_deleted', \
             ordinary_deleted_at=now(), visibility='draft' WHERE id={book}"
        ),
    );
    assert_eq!(
        private_get(app, &format!("/books/{book}"), None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        private_get(app, &format!("/read/{free_chapter}"), Some(buyer_cookie))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "ordinary-deleted free chapters have no public reader URL"
    );

    let archive = format!("/me/library/{book}/saved");
    let archive_response = private_get(app, &archive, Some(buyer_cookie)).await;
    assert_eq!(archive_response.status(), StatusCode::OK);
    assert_eq!(
        archive_response.headers()[header::CACHE_CONTROL],
        "no-store"
    );
    let toc = html_body(archive_response).await;
    assert!(toc.contains(&format!("href=\"/me/library/saved/{free_chapter}\"")));
    assert!(toc.contains("Сохранённая бесплатная редакция 1"));
    assert!(toc.contains(&format!(
        "href=\"/read/paid/{chapter}?purchase_id={purchase_id}\""
    )));
    assert!(toc.contains("Купленная редакция 1"));
    assert!(
        toc.contains("&lt;img"),
        "free chapter titles must be escaped"
    );
    assert!(!toc.contains("<img src=x"));
    assert!(!toc.contains(&format!("/reader/{locked_chapter}")));
    assert!(!toc.contains(&format!("/read/paid/{locked_chapter}")));
    assert!(!toc.contains(&format!("/me/library/saved/{draft_chapter}")));
    assert!(!toc.contains("UNPINNED FREE V2"));
    assert!(
        !toc.contains(MARKDOWN),
        "private TOC cannot expose paid body"
    );

    let saved = private_get(
        app,
        &format!("/me/library/saved/{free_chapter}"),
        Some(buyer_cookie),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let free_html = html_body(saved).await;
    assert!(free_html.contains("Real public free content"));
    assert!(!free_html.contains("UNPINNED FREE V2"));
    let buyer_library = html_body(private_get(app, "/me/library", Some(buyer_cookie)).await).await;
    assert!(buyer_library.contains("Удалена"));
    assert!(buyer_library.contains(&archive));

    // The existing signed session is not enough: after completed deletion,
    // recover both immutable editions through a *new* real HTTP login.
    let fresh = fresh_buyer_login(app).await;
    assert!(fresh.session != buyer_cookie, "login must create a new session");
    let recovered = private_get(app, &archive, Some(&fresh.session)).await;
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered_toc = html_body(recovered).await;
    assert!(recovered_toc.contains(&format!(
        "href=\"/me/library/saved/{free_chapter}\""
    )));
    assert!(recovered_toc.contains(&format!(
        "href=\"/read/paid/{chapter}?purchase_id={purchase_id}\""
    )));
    assert!(!recovered_toc.contains("UNPINNED FREE V2"));
    assert!(!recovered_toc.contains(MARKDOWN));
    let recovered_free = private_get(
        app,
        &format!("/me/library/saved/{free_chapter}"),
        Some(&fresh.session),
    )
    .await;
    assert_eq!(recovered_free.status(), StatusCode::OK);
    let recovered_text = html_body(recovered_free).await;
    assert!(recovered_text.contains("Real public free content"));
    assert!(!recovered_text.contains("UNPINNED FREE V2"));
    let recovered_paid = paid_page(app, chapter, purchase_id, &fresh.session).await;
    assert_eq!(recovered_paid.status(), StatusCode::OK);
    assert!(html_body(recovered_paid)
        .await
        .contains("data-paid-chapter-reader"));
    let fresh_library = html_body(private_get(app, "/me/library", Some(&fresh.session)).await).await;
    assert!(fresh_library.contains("Удалена"));
    assert!(fresh_library.contains(&archive));

    let outsider_toc = html_body(private_get(app, &archive, Some(stranger_cookie)).await).await;
    assert!(!outsider_toc.contains(&format!("/read/paid/{chapter}?purchase_id=")));
    assert!(!outsider_toc.contains(&format!("/me/library/saved/{free_chapter}")));
    assert_eq!(
        private_get(
            app,
            &format!("/me/library/saved/{free_chapter}"),
            Some(stranger_cookie),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        post_grant(
            app,
            chapter,
            Some(stranger_cookie),
            Some(csrf),
            ORIGIN,
            "{}",
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    // A live 18+/21+ revocation closes BOTH archive routes and the paid grant.
    // Reverting the test fixture returns to the original basic rights before
    // the real MinIO grant is issued; it cannot create a new purchase.
    for tier in ["adult", "pro_adult"] {
        pg(
            url,
            &format!("UPDATE books SET access_level='{tier}' WHERE id={book}"),
        );
        let denied_archive = html_body(private_get(app, &archive, Some(buyer_cookie)).await).await;
        assert!(!denied_archive.contains(&format!("/me/library/saved/{free_chapter}")));
        assert!(!denied_archive.contains(&format!("/read/paid/{chapter}?purchase_id=")));
        assert_ne!(
            private_get(
                app,
                &format!("/me/library/saved/{free_chapter}"),
                Some(buyer_cookie),
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_ne!(
            post_grant(app, chapter, Some(buyer_cookie), Some(csrf), ORIGIN, "{}")
                .await
                .status(),
            StatusCode::OK
        );
        assert_ne!(
            private_get(app, &archive, Some(&fresh.session))
                .await
                .status(),
            StatusCode::OK
        );
        assert_ne!(
            post_grant(app, chapter, Some(&fresh.session), Some(&fresh.csrf), ORIGIN, "{}")
                .await
                .status(),
            StatusCode::OK
        );
    }
    pg(
        url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );
    let archived_reader = paid_page(app, chapter, purchase_id, buyer_cookie).await;
    assert_eq!(archived_reader.status(), StatusCode::OK);
    let archived_html = html_body(archived_reader).await;
    assert!(archived_html.contains("data-paid-chapter-reader"));
    assert!(!archived_html.contains("data-toc-state=\"free\""));
    assert!(!archived_html.contains("data-toc-state=\"locked\""));
    // This newly signed grant belongs to the second login and is consumed by
    // the existing real MinIO/WebCrypto fixture after the helper returns.
    let authorized = post_grant(
        app,
        chapter,
        Some(&fresh.session),
        Some(&fresh.csrf),
        ORIGIN,
        "{}",
    )
    .await;
    assert_eq!(authorized.status(), StatusCode::OK);
    let grant = decode(authorized).await;
    assert_eq!(grant["version_id"], v1.to_string());
    assert!(!grant.to_string().contains(MARKDOWN));
    grant
}

/// Following the existing refund test, the free pin remains but the refunded
/// paid edition must disappear from the same private mixed Book TOC.
pub(super) async fn verify_refund_archive(
    app: &Router,
    book: i64,
    free_chapter: i64,
    purchased: (i64, i64),
    buyer_cookie: &str,
) {
    let (chapter, purchase_id) = purchased;
    let toc = html_body(
        private_get(
            app,
            &format!("/me/library/{book}/saved"),
            Some(buyer_cookie),
        )
        .await,
    )
    .await;
    assert!(toc.contains(&format!("/me/library/saved/{free_chapter}")));
    assert!(!toc.contains(&format!("/read/paid/{chapter}?purchase_id={purchase_id}")));
}
