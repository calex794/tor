use super::*;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn signal_windows_are_equal_completed_hours_with_a_full_proof_grace() {
    let f = Fixture::new(1).await;
    sqlx::query("WITH anchor AS (SELECT date_trunc('hour',(now()-interval '15 minutes') AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS until) INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views) SELECT until-offset_hours*interval '1 hour',$1,'catalog',50,40,value FROM anchor CROSS JOIN (VALUES(1,10),(24*7,3),(24*7+1,4),(24*14,2),(0,20),(24*14+1,30)) v(offset_hours,value)")
        .bind(f.books[0]).execute(&f.pool).await.unwrap();
    let report = f.store.report_closed(&f.filter()).await.unwrap();
    assert_eq!(
        (report.current.opportunities, report.current.views),
        (100, 13)
    );
    assert_eq!(
        (report.previous.opportunities, report.previous.views),
        (100, 6)
    );
    f.cleanup().await;
}
#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn report_distinguishes_missing_measurement_and_deduplicates_groups_before_pagination() {
    let f = Fixture::new(53).await;
    let mut tx = f.pool.begin().await.unwrap();
    for (i, opps, rendered, views) in [(0, 25, 20, 0), (1, 100, 20, 1), (2, 25, 25, 5)] {
        sqlx::query("INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views) VALUES(date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC',$1,'home',$2,$3,$4)").bind(f.books[i]).bind(opps as i64).bind(rendered as i64).bind(views as i64).execute(&mut *tx).await.unwrap();
    }
    sqlx::query("INSERT INTO flow_observation_hourly(bucket,book_id,surface,opportunities,rendered,views) VALUES(((now() AT TIME ZONE 'UTC')::date-7)::timestamp AT TIME ZONE 'UTC',$1,'home',25,25,3)").bind(f.books[0]).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO flow_observation_hourly(bucket,book_id,surface,campaign_id,slot,opportunities,rendered,views) VALUES(date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC',$1,'home',811,'home_featured',4,2,1)").bind(f.books[2]).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let q = f.filter();
    let report = f.store.report(&q).await.unwrap();
    assert_eq!(report.books, 53);
    assert_eq!(report.rows.len(), 50);
    assert_eq!(report.low_observed_books, 1);
    assert_eq!(report.unknown_books, 51);
    assert_eq!(
        (
            report.current.opportunities,
            report.current.rendered,
            report.current.views
        ),
        (154, 67, 7)
    );
    assert_eq!(report.previous.views, 3);
    assert_eq!(report.rows[3].counts.known_views(), None);
    assert_eq!(report.rows[0].counts.known_views(), Some(0));
    let group = report
        .groups
        .iter()
        .find(|g| g.kind == "genre" && g.label == f.label)
        .unwrap();
    assert_eq!(group.books, 53);
    assert_eq!(group.counts, report.current);
    let mut next = q.clone();
    next.after = report.next.unwrap();
    let second = f.store.report(&next).await.unwrap();
    assert_eq!(second.rows.len(), 3);
    assert!(second.next.is_none());
    assert_eq!(second.current, report.current);
    next.after = 0;
    next.class = Some(ObservationClass::LowObserved);
    let low = f.store.report(&next).await.unwrap();
    assert_eq!(low.rows.len(), 1);
    assert_eq!(low.rows[0].id, f.books[0]);
    assert_eq!(low.current, report.current);
    let mut campaign = q.clone();
    campaign.editorial = Some(true);
    campaign.campaign = Some(811);
    let promotion = f
        .store
        .report_campaign_book(&campaign, f.books[2])
        .await
        .unwrap();
    assert_eq!(promotion.books, 1);
    assert_eq!(promotion.rows.len(), 1);
    assert_eq!(promotion.rows[0].id, f.books[2]);
    assert_eq!(promotion.current.opportunities, 4);
    assert_eq!(promotion.current.views, 1);
    campaign.campaign = Some(812);
    let untouched = f
        .store
        .report_campaign_book(&campaign, f.books[0])
        .await
        .unwrap();
    assert_eq!(untouched.books, 1);
    assert_eq!(untouched.unknown_books, 1);
    assert_eq!(untouched.current.known_views(), None);
    assert_eq!(
        f.store.report(&campaign).await.err(),
        Some(ExposureError::Invalid)
    );
    campaign.editorial = Some(false);
    campaign.campaign = None;
    assert_eq!(f.store.report(&campaign).await.unwrap().current.views, 6);
    let readonly = PgPoolOptions::new()
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect(&current_analytics::url("TEST_DATABASE_URL", "tor_app"))
        .await
        .unwrap();
    assert_eq!(
        PgExposure::new(readonly.clone())
            .report(&q)
            .await
            .unwrap()
            .current,
        report.current
    );
    readonly.close().await;
    f.book_change("visibility_status='hidden'").await;
    let hidden = f.store.report(&q).await.unwrap();
    assert_eq!(hidden.books, 52);
    assert_eq!(hidden.low_observed_books, 0);
    assert_eq!(hidden.current.opportunities, 129);
    assert_eq!(hidden.previous.views, 0);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn minted_editorial_context_is_bound_and_retired_tables_are_not_required() {
    let f = Fixture::new(1).await;
    let mut card = f.card(0);
    card.editorial = Some(EditorialPlacement {
        campaign_id: 17,
        revision: 4,
        slot: PromotionSlot::HomeFeatured,
    });
    let (proof, issued) = issue(card).unwrap();
    let mut retired = f.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE rec_subjects,rec_reader_signals,rec_assignments,rec_quality_daily IN ACCESS EXCLUSIVE MODE").execute(&mut *retired).await.unwrap();
    f.store.register(&[issued]).await.unwrap();
    let context = f.store.describe(&proof).await.unwrap();
    assert_eq!(context.card, card);
    f.store
        .record(&event(&proof, EventKind::Rendered), &context, None)
        .await
        .unwrap();
    assert_eq!(
        f.store.report(&f.filter()).await.unwrap().current.rendered,
        1
    );
    retired.rollback().await.unwrap();
    let (_, too_early) = issue(f.card(0)).unwrap();
    let nonce = issue(f.card(0)).unwrap();
    f.store.register(&[too_early, nonce.1]).await.unwrap();
    let context = f.store.describe(&nonce.0).await.unwrap();
    assert_eq!(
        f.store
            .record(&event(&nonce.0, EventKind::View), &context, None)
            .await,
        Err(ExposureError::TooEarly)
    );
    assert_eq!(
        f.store.describe(&issue(f.card(0)).unwrap().0).await.err(),
        Some(ExposureError::Missing)
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn attention_matrix_matches_totals_and_scopes_without_reader_attribution() {
    let f = Fixture::new(3).await;
    let entries = [
        (
            f.books[0], "home", 0_i64, "", 10_i64, 8_i64, 3_i64, 2_i64, 1_i32,
        ),
        (f.books[1], "reader_end", 0, "", 40, 35, 5, 2, 1),
        (
            f.books[2],
            "reader_end",
            891,
            "chapter_end_free",
            8,
            8,
            3,
            4,
            1,
        ),
        (f.books[1], "reader_end", 0, "", 20, 18, 2, 1, 192),
    ];
    for (book, surface, campaign, slot, opportunities, rendered, views, clicks, hours) in entries {
        sqlx::query(
            "INSERT INTO flow_observation_hourly
             (bucket,book_id,surface,campaign_id,slot,opportunities,rendered,views,clicks)
             VALUES (
               (date_trunc('hour',(now()-interval '15 minutes') AT TIME ZONE 'UTC')
                 AT TIME ZONE 'UTC')-make_interval(hours=>$9),
               $1,$2,$3,$4,$5,$6,$7,$8
             )",
        )
        .bind(book)
        .bind(surface)
        .bind(campaign)
        .bind(slot)
        .bind(opportunities)
        .bind(rendered)
        .bind(views)
        .bind(clicks)
        .bind(hours)
        .execute(&f.pool)
        .await
        .unwrap();
    }
    let filter = f.filter();
    let whole = f.store.report_closed(&filter).await.unwrap();
    assert_eq!(
        (
            whole.current.opportunities,
            whole.current.views,
            whole.current.clicks
        ),
        (58, 11, 8)
    );
    assert_eq!(
        (whole.previous.opportunities, whole.previous.views),
        (20, 2)
    );
    assert_eq!(whole.placements.len(), 3);
    let find = |surface: ExposureSurface, editorial: bool| {
        whole
            .placements
            .iter()
            .find(|r| r.surface == surface && r.editorial == editorial)
            .unwrap()
    };
    assert_eq!(find(ExposureSurface::Home, false).current.opportunities, 10);
    assert_eq!(find(ExposureSurface::ReaderEnd, false).current.views, 5);
    assert_eq!(find(ExposureSurface::ReaderEnd, false).previous.views, 2);
    assert_eq!(find(ExposureSurface::ReaderEnd, true).current.views, 3);
    assert_eq!(
        whole
            .placements
            .iter()
            .map(|r| r.current.views)
            .sum::<i64>(),
        whole.current.views
    );
    assert_eq!(
        whole
            .placements
            .iter()
            .map(|r| r.previous.views)
            .sum::<i64>(),
        whole.previous.views
    );

    let mut by_surface = filter.clone();
    by_surface.surface = Some(ExposureSurface::ReaderEnd);
    let reader = f.store.report_closed(&by_surface).await.unwrap();
    assert_eq!(reader.current.views, 8);
    assert_eq!(reader.placements.len(), 2);

    let mut by_source = filter.clone();
    by_source.editorial = Some(true);
    let editorial = f.store.report_closed(&by_source).await.unwrap();
    assert_eq!(editorial.current.views, 3);
    assert_eq!(editorial.placements.len(), 1);
    assert!(editorial.placements[0].editorial);

    by_source.surface = Some(ExposureSurface::Home);
    let empty = f.store.report_closed(&by_source).await.unwrap();
    assert!(empty.placements.is_empty());
    assert_eq!(empty.current.views, 0);

    let mut paged = filter.clone();
    paged.after = f.books[1];
    let page = f.store.report_closed(&paged).await.unwrap();
    assert_eq!(page.placements.len(), 3);
    assert_eq!(page.current.views, whole.current.views);

    f.book_change("visibility_status='hidden'").await;
    let hidden = f.store.report_closed(&filter).await.unwrap();
    assert_eq!(hidden.current.views, 8);
    assert!(hidden
        .placements
        .iter()
        .all(|p| p.surface != ExposureSurface::Home));
    f.cleanup().await;
}
