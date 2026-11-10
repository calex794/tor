//! Observe real accepted navigation without requiring every fail-open HTML
//! response to have telemetry. Only a proven pre-enqueue scope miss is retried.
use super::*;

#[derive(Clone, Copy, Debug)]
struct QueueCounts {
    offered: u64,
    saved: u64,
    dropped: u64,
    scope_timeouts: u64,
    failures: u64,
}

async fn queue_counts(h: &Harness, administrator: &Browser) -> QueueCounts {
    let response = administrator
        .get(&h.app, "/admin/audience/visits?days=1")
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    private(&response);
    let html = text(response).await;
    let section = html
        .split("Состояние текущего сборщика")
        .nth(1)
        .expect("real private collector diagnostics")
        .split("</section>")
        .next()
        .unwrap();
    let count = |label: &str| -> u64 {
        section
            .split(&format!("<dt>{label}</dt><dd>"))
            .nth(1)
            .unwrap_or_else(|| panic!("missing collector metric: {label}"))
            .split('<')
            .next()
            .unwrap()
            .parse()
            .expect("numeric collector metric")
    };
    QueueCounts {
        offered: count("Предложено очереди"),
        saved: count("Подтверждено новых записей"),
        dropped: count("Переполнения очереди или записи без подтверждения"),
        scope_timeouts: count("Проверка согласия недоступна или превысила время ожидания"),
        failures: count("Неуспешные попытки записи"),
    }
}

fn receipts(h: &Harness, actor: i64, book: i64, kind: i16) -> i64 {
    let surface = if kind == 0 { 2 } else { 3 };
    pg(
        &h.url,
        &format!(
            "SELECT count(*) FROM flow_visit_receipts r
             JOIN rec_subjects s USING(subject_id)
             WHERE s.user_id={actor} AND r.book_id={book}
               AND r.book_kind={kind} AND r.surface={surface}"
        ),
    )
    .parse()
    .unwrap()
}

pub(super) async fn captured_book_page(
    h: &Harness,
    browser: &Browser,
    administrator: &Browser,
    actor: i64,
    path: &str,
    book: i64,
    kind: i16,
) -> Response {
    assert!(book > 0 && matches!(kind, 0 | 1));
    let before_pages = own_pages(&h.url, actor);
    let before_receipts = receipts(h, actor, book, kind);
    for attempt in 1..=5 {
        let before = queue_counts(h, administrator).await;
        let response = browser.get(&h.app, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let after = queue_counts(h, administrator).await;
        let pages = own_pages(&h.url, actor);
        let evidence = receipts(h, actor, book, kind);
        let diagnostic = format!(
            "{path}, attempt={attempt}, pages={before_pages}->{pages}, \
             receipts={before_receipts}->{evidence}, saved={}->{}, before={before:?}, after={after:?}",
            before.saved, after.saved
        );
        assert_eq!(after.dropped, before.dropped, "{diagnostic}");
        assert_eq!(after.failures, before.failures, "{diagnostic}");
        if after.offered == before.offered && after.scope_timeouts == before.scope_timeouts + 1 {
            // HTTP succeeded while its optional 25ms identity/consent lookup
            // was unavailable. No observation exists to duplicate on retry.
            assert_eq!(pages, before_pages, "{diagnostic}");
            assert_eq!(evidence, before_receipts, "{diagnostic}");
            eprintln!("FLOW pre-enqueue scope miss: {diagnostic}");
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        assert_eq!(after.offered, before.offered + 1, "{diagnostic}");
        assert_eq!(after.scope_timeouts, before.scope_timeouts, "{diagnostic}");
        // Once offered, wait for that same event. A slow write, dropped event,
        // wrong book context or duplicate must fail rather than cause another GET.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let pages = own_pages(&h.url, actor);
            assert!(pages <= before_pages + 1, "duplicate capture: {diagnostic}");
            if pages == before_pages + 1 {
                assert_eq!(
                    receipts(h, actor, book, kind),
                    before_receipts + 1,
                    "{diagnostic}"
                );
                let settled = queue_counts(h, administrator).await;
                assert_eq!(settled.dropped, before.dropped, "{path}: {settled:?}");
                assert_eq!(settled.failures, before.failures, "{path}: {settled:?}");
                return response;
            }
            if tokio::time::Instant::now() >= deadline {
                let settled = queue_counts(h, administrator).await;
                panic!("offered FLOW page was not saved: {diagnostic}; final={settled:?}, pages={pages}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    panic!("FLOW scope remained unavailable for five real navigations: {path}");
}
