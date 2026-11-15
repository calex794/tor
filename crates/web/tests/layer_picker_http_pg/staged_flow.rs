{
    // The staged grid survives another search/page and one final confirmation.
    let draft_version: i64 = pg(
        &url,
        &format!("SELECT current_version_id FROM commerce_layers WHERE id={id}"),
    )
    .parse()
    .unwrap();
    let extra: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:d05:staged-extra:{nonce}','ru','Another package Book','public',{owner})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let stage_remove = format!(
        "operation=stage&expected_version_id={draft_version}&draft_revision=0&visible_csv={authors_book}&after=0&filter=all&nav=search&csrf_token={csrf}"
    );
    assert_eq!(
        request(
            &app,
            "POST",
            &destination,
            &cookie,
            "https://evil.invalid",
            &stage_remove
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let stage_one = request(&app, "POST", &destination, &cookie, ORIGIN, &stage_remove).await;
    assert_eq!(stage_one.status(), StatusCode::SEE_OTHER);
    let stage_add = format!(
        "operation=stage&expected_version_id={draft_version}&draft_revision=1&visible_csv={extra}&chosen_{extra}=1&after=0&filter=all&nav=filter_included&csrf_token={csrf}"
    );
    assert_eq!(
        request(&app, "POST", &destination, &cookie, ORIGIN, &stage_add)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let included_page = body(
        request(
            &app,
            "GET",
            &format!("{destination}?filter=included"),
            &cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(included_page.contains("Another package Book"));
    assert!(included_page.contains(&format!("name=\"chosen_{extra}\" value=\"1\" checked")));
    assert!(included_page.contains("name=\"draft_revision\" value=\"2\""));
    let review_draft = format!(
        "operation=stage&expected_version_id={draft_version}&draft_revision=2&visible_csv={extra}&chosen_{extra}=1&after=0&filter=included&nav=review&csrf_token={csrf}"
    );
    let review = request(&app, "POST", &destination, &cookie, ORIGIN, &review_draft).await;
    assert_eq!(review.status(), StatusCode::OK);
    let review_html = body(review).await;
    assert!(review_html.contains("Добавим: 1"));
    assert!(review_html.contains("Уберём: 1"));
    assert!(review_html.contains("name=\"draft_revision\" value=\"3\""));
    let invalid_apply = format!(
        "operation=apply_draft&expected_version_id={draft_version}&draft_revision=2&active_buyers=0&lifetime_buyers=0&confirm_composition=yes&csrf_token={csrf}"
    );
    assert_eq!(
        request(&app, "POST", &destination, &cookie, ORIGIN, &invalid_apply)
            .await
            .status(),
        StatusCode::CONFLICT,
    );
    let apply_draft = invalid_apply.replace("draft_revision=2", "draft_revision=3");
    assert_eq!(
        request(&app, "POST", &destination, &cookie, ORIGIN, &apply_draft)
            .await
            .status(),
        StatusCode::SEE_OTHER,
    );
    let saved_page = request(
        &app,
        "GET",
        &format!("{destination}?updated=1"),
        &cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(saved_page.status(), StatusCode::OK);
    let saved_html = body(saved_page).await;
    assert!(saved_html.contains("Изменения пакета сохранены"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM commerce_layer_composition_drafts WHERE layer_id={id}")
        ),
        "0"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM commerce_layer_version_books
                 WHERE layer_version_id=(SELECT current_version_id FROM commerce_layers WHERE id={id})
                   AND book_id={extra}"
            )
        ),
        "1"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM commerce_layer_version_books
                 WHERE layer_version_id={draft_version} AND book_id={authors_book}"
            )
        ),
        "1",
        "historical version remains untouched",
    );
}
