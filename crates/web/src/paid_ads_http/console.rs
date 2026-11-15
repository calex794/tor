use super::*;
use axum::{
    extract::{rejection::FormRejection, Form, Path, Query, State},
    response::Redirect,
};
use chrono::{NaiveDate, TimeDelta};
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageQuery {
    pub book: Option<i64>,
    pub daily_before: Option<i64>,
    pub chapter_before: Option<i64>,
    pub publisher_before: Option<i64>,
    pub changed: Option<String>,
}
pub(super) async fn page(
    State(s): State<Runtime>,
    h: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        let (offers, packages, books, daily, chapter, publisher) = tokio::try_join!(
            s.catalog.daily_offers(),
            s.catalog.packages(),
            s.catalog.owned_books(&a),
            s.catalog.campaigns(&a, AdProduct::Daily, q.daily_before),
            s.catalog
                .campaigns(&a, AdProduct::Chapter, q.chapter_before),
            s.catalog.publisher(&a, q.publisher_before)
        )
        .map_err(status)?;
        let (csrf, cookies) = s.cookies.csrf_for_page(&h);
        Ok::<_, StatusCode>(secure(
            render::console(render::ConsoleView {
                q: &q,
                csrf: &csrf,
                flags: s.flags,
                offers: &offers,
                packages: &packages,
                books: &books,
                daily: &daily,
                chapter: &chapter,
                publisher: &publisher,
            }),
            &cookies,
        ))
    }
    .await;
    result.unwrap_or_else(native_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DailyForm {
    csrf_token: String,
    command_key: String,
    book_id: i64,
    offer: String,
    days: u32,
    start_date: String,
    accept_terms: bool,
}
pub(super) async fn daily(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Form<DailyForm>, FormRejection>,
) -> Response {
    let result = async {
        let actor = actor(&s, &h, false).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(&s, &h, &b.csrf_token)?;
        if !b.accept_terms || b.days == 0 || b.days > 366 {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
        let parts = b.offer.split(':').collect::<Vec<_>>();
        if parts.len() != 3 {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
        let start = NaiveDate::parse_from_str(&b.start_date, "%Y-%m-%d")
            .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        let end = start
            .checked_add_signed(TimeDelta::days(i64::from(b.days)))
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
        let purchase = AdPurchase::Daily(DailyPurchase {
            book_id: b.book_id,
            offer_id: parts[0]
                .parse()
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
            offer_revision: parts[1]
                .parse()
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
            placement: parts[2]
                .parse()
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
            starts_at: format!("{start}T00:00:00Z"),
            ends_at: format!("{end}T00:00:00Z"),
            total_days: b.days,
            prepaid_days: b.days,
        });
        s.service()
            .purchase(&AdPurchaseCommand {
                actor,
                command_key: b.command_key,
                purchase,
            })
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(Redirect::to("/me/ads?changed=purchased"), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChapterForm {
    csrf_token: String,
    command_key: String,
    book_id: i64,
    package: String,
    accept_terms: bool,
}
pub(super) async fn chapter(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Form<ChapterForm>, FormRejection>,
) -> Response {
    let result = async {
        let actor = actor(&s, &h, false).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(&s, &h, &b.csrf_token)?;
        if !b.accept_terms {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
        let (id, rev) = b
            .package
            .split_once(':')
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
        let purchase = AdPurchase::Chapter(ChapterPurchase {
            book_id: b.book_id,
            package_id: id.parse().map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
            expected_package_revision: rev.parse().map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
        });
        s.service()
            .purchase(&AdPurchaseCommand {
                actor,
                command_key: b.command_key,
                purchase,
            })
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(Redirect::to("/me/ads?changed=purchased"), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CancelForm {
    pub csrf_token: String,
    pub command_key: String,
    pub expected_revision: i64,
    pub reason: String,
}
pub(super) async fn cancel(
    State(s): State<Runtime>,
    h: HeaderMap,
    Path((p, id)): Path<(String, i64)>,
    b: Result<Form<CancelForm>, FormRejection>,
) -> Response {
    cancel_form(&s, &h, &p, id, b, false).await
}
pub(super) async fn cancel_form(
    s: &Runtime,
    h: &HeaderMap,
    p: &str,
    id: i64,
    b: Result<Form<CancelForm>, FormRejection>,
    admin: bool,
) -> Response {
    let result = async {
        let actor = actor(s, h, admin).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(s, h, &b.csrf_token)?;
        let receipt = s
            .service()
            .cancel(&AdCancelCommand {
                actor,
                product: product(p)?,
                campaign_id: AdCampaignId::new(id).map_err(status)?,
                expected_revision: b.expected_revision,
                command_key: b.command_key,
                reason: b.reason,
            })
            .await
            .map_err(status)?;
        let next = if admin {
            "/admin/promotion/paid"
        } else if receipt.pending {
            "/me/ads?changed=cancelling"
        } else {
            "/me/ads?changed=cancelled"
        };
        Ok::<_, StatusCode>(secure(Redirect::to(next), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PublisherForm {
    csrf_token: String,
    expected_revision: i64,
    enabled: bool,
}
pub(super) async fn publisher_settings(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Form<PublisherForm>, FormRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(&s, &h, &b.csrf_token)?;
        s.catalog
            .publisher_settings(&a, b.expected_revision, b.enabled)
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(Redirect::to("/me/ads?changed=settings"), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreditForm {
    csrf_token: String,
    pub command_key: String,
}
pub(super) async fn publisher_credit(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Form<CreditForm>, FormRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(&s, &h, &b.csrf_token)?;
        if b.command_key.len() != 43 {
            return Err(StatusCode::UNPROCESSABLE_ENTITY);
        }
        s.funding.mature_publisher(&a, 100).await.map_err(status)?;
        s.funding
            .release_publisher(&a, &b.command_key, 100)
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(Redirect::to("/me/ads?changed=credited"), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
