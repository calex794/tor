use super::wire::{id as decimal_id, json_status, utc_day};
use super::*;
use axum::{
    extract::{rejection::JsonRejection, Path, Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DailyBody {
    book_id: String,
    offer_id: String,
    offer_revision: i64,
    placement_type: String,
    starts_at: String,
    ends_at: String,
    total_days: u32,
    prepaid_days: u32,
}
impl DailyBody {
    fn purchase(self) -> Result<AdPurchase, ApiError> {
        Ok(AdPurchase::Daily(DailyPurchase {
            book_id: decimal_id(&self.book_id)?,
            offer_id: decimal_id(&self.offer_id)?,
            offer_revision: self.offer_revision,
            placement: self
                .placement_type
                .parse()
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?,
            starts_at: utc_day(&self.starts_at)?,
            ends_at: utc_day(&self.ends_at)?,
            total_days: self.total_days,
            prepaid_days: self.prepaid_days,
        }))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChapterBody {
    promoted_book_id: String,
    package_id: String,
    expected_package_revision: i64,
}
impl ChapterBody {
    fn purchase(self) -> Result<AdPurchase, ApiError> {
        Ok(AdPurchase::Chapter(ChapterPurchase {
            book_id: decimal_id(&self.promoted_book_id)?,
            package_id: decimal_id(&self.package_id)?,
            expected_package_revision: self.expected_package_revision,
        }))
    }
}
pub(super) async fn daily(
    State(s): State<Runtime>,
    h: HeaderMap,
    body: Result<Json<DailyBody>, JsonRejection>,
) -> Response {
    let purchase = body.map_err(json_status).and_then(|Json(b)| b.purchase());
    purchase_response(&s, &h, purchase).await
}
pub(super) async fn chapter(
    State(s): State<Runtime>,
    h: HeaderMap,
    body: Result<Json<ChapterBody>, JsonRejection>,
) -> Response {
    purchase_response(
        &s,
        &h,
        body.map_err(json_status).and_then(|Json(b)| b.purchase()),
    )
    .await
}
async fn purchase_response(
    s: &Runtime,
    h: &HeaderMap,
    purchase: Result<AdPurchase, ApiError>,
) -> Response {
    let result=async{
  let actor=actor(s,h,false).await?;api_csrf(s,h)?;let purchase=purchase?;let command_key=command_key(h)?;
  let receipt=s.service().purchase(&AdPurchaseCommand{actor,command_key,purchase}).await.map_err(ad_error)?;
  Ok::<_,ApiError>(secure((if receipt.replayed{StatusCode::OK}else{StatusCode::CREATED},Json(json!({
   "campaign_id":receipt.campaign_id.get().to_string(),"product":receipt.product.as_str(),"charged_minor":receipt.charged.get(),
   "balance_after_minor":receipt.balance_after.get(),"replayed":receipt.replayed}))),&[]))
 }.await;
    result.unwrap_or_else(error)
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cursor {
    before: Option<i64>,
}
pub(super) async fn offers(State(s): State<Runtime>) -> Response {
    match s.catalog.daily_offers().await {
        Ok(rows) => secure(
            Json(json!({"offers":rows,"purchase_enabled":s.flags.daily_purchase})),
            &[],
        ),
        Err(e) => error(status(e)),
    }
}
pub(super) async fn packages(State(s): State<Runtime>) -> Response {
    match s.catalog.packages().await {
        Ok(rows) => secure(
            Json(json!({"packages":rows,"purchase_enabled":s.flags.chapter_purchase})),
            &[],
        ),
        Err(e) => error(status(e)),
    }
}
pub(super) async fn daily_campaigns(
    State(s): State<Runtime>,
    h: HeaderMap,
    Query(q): Query<Cursor>,
) -> Response {
    campaigns(&s, &h, AdProduct::Daily, q.before).await
}
pub(super) async fn chapter_campaigns(
    State(s): State<Runtime>,
    h: HeaderMap,
    Query(q): Query<Cursor>,
) -> Response {
    campaigns(&s, &h, AdProduct::Chapter, q.before).await
}
async fn campaigns(s: &Runtime, h: &HeaderMap, p: AdProduct, before: Option<i64>) -> Response {
    let result = async {
        let a = actor(s, h, false).await?;
        let rows = s.catalog.campaigns(&a, p, before).await.map_err(ad_error)?;
        let next = if rows.len() == 100 {
            rows.last().map(|r| r.id.to_string())
        } else {
            None
        };
        Ok::<_, ApiError>(secure(
            Json(json!({"campaigns":rows,"next_before":next})),
            &[],
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
pub(super) async fn campaign(
    State(s): State<Runtime>,
    h: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        let c = s.catalog.campaign(&a, id).await.map_err(ad_error)?;
        Ok::<_, ApiError>(secure(Json(c), &[]))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CancelBody {
    expected_revision: i64,
    #[serde(default = "cancel_reason")]
    reason: String,
}
pub(super) async fn cancel_daily(
    State(s): State<Runtime>,
    h: HeaderMap,
    Path(id): Path<i64>,
    b: Result<Json<CancelBody>, JsonRejection>,
) -> Response {
    cancel_response(&s, &h, id, AdProduct::Daily, b).await
}
pub(super) async fn cancel_chapter(
    State(s): State<Runtime>,
    h: HeaderMap,
    Path(id): Path<i64>,
    b: Result<Json<CancelBody>, JsonRejection>,
) -> Response {
    cancel_response(&s, &h, id, AdProduct::Chapter, b).await
}
async fn cancel_response(
    s: &Runtime,
    h: &HeaderMap,
    id: i64,
    p: AdProduct,
    b: Result<Json<CancelBody>, JsonRejection>,
) -> Response {
    let result = async {
        let actor = actor(s, h, false).await?;
        api_csrf(s, h)?;
        let Json(b) = b.map_err(json_status)?;
        let r = s
            .service()
            .cancel(&AdCancelCommand {
                actor,
                product: p,
                campaign_id: AdCampaignId::new(id).map_err(ad_error)?,
                expected_revision: b.expected_revision,
                command_key: command_key(h)?,
                reason: b.reason,
            })
            .await
            .map_err(ad_error)?;
        Ok::<_, ApiError>(secure(
            (
                if r.pending {
                    StatusCode::ACCEPTED
                } else {
                    StatusCode::OK
                },
                Json(json!({"campaign_id":id.to_string(),
   "refunded_minor":r.refunded.get(),"pending":r.pending,"replayed":r.replayed})),
            ),
            &[],
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImpressionBody {
    campaign_id: String,
    generation_id: String,
    assignment_id: String,
    viewability_token: String,
    observed_visible_fraction: f64,
    observed_duration_ms: u64,
}
pub(super) async fn impression(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Json<ImpressionBody>, JsonRejection>,
) -> Response {
    let result = async {
        let actor = actor(&s, &h, false).await?;
        api_csrf(&s, &h)?;
        let Json(b) = b.map_err(json_status)?;
        if !b.observed_visible_fraction.is_finite() || !(0.0..=1.0).contains(&b.observed_visible_fraction){return Err(StatusCode::UNPROCESSABLE_ENTITY.into());}
        let r = s
            .service()
            .impression(&AdImpressionClaim {
                actor,
                context:AdDeliveryContext{campaign_id:AdCampaignId::new(decimal_id(&b.campaign_id)?).map_err(ad_error)?,
                    generation_id:AdGenerationId::new(decimal_id(&b.generation_id)?).map_err(ad_error)?,
                    assignment_id:AdAssignmentId::new(decimal_id(&b.assignment_id)?).map_err(ad_error)?},
                viewability_token: b.viewability_token,
                visible_percent:(b.observed_visible_fraction*100.0).floor() as u16,
                duration_ms:b.observed_duration_ms,
            })
            .await
            .map_err(ad_error)?;
        Ok::<_, ApiError>(secure(
            Json(
                json!({"impression_id":r.impression_id.get().to_string(),"campaign_id":r.campaign_id.get().to_string(),
   "ordinal":r.ordinal,"gross_minor":r.gross.get(),"replayed":r.replayed}),
            ),
            &[],
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
pub(super) async fn publisher(
    State(s): State<Runtime>,
    h: HeaderMap,
    Query(q): Query<Cursor>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        let (balance, credits) = s.catalog.publisher(&a, q.before).await.map_err(ad_error)?;
        let next = if credits.len() == 100 {
            credits.last().map(|c| c.id.to_string())
        } else {
            None
        };
        Ok::<_, ApiError>(secure(
            Json(json!({"balance":balance,"credits":credits,"next_before":next})),
            &[],
        ))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingsBody {
    expected_revision: i64,
    enabled: bool,
}
pub(super) async fn publisher_settings(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Json<SettingsBody>, JsonRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        api_csrf(&s, &h)?;
        let Json(b) = b.map_err(json_status)?;
        s.catalog
            .publisher_settings(&a, b.expected_revision, b.enabled)
            .await
            .map_err(ad_error)?;
        Ok::<_, ApiError>(secure(StatusCode::NO_CONTENT, &[]))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreditBody {}
pub(super) async fn publisher_credit(
    State(s): State<Runtime>,
    h: HeaderMap,
    body: Result<Json<CreditBody>, JsonRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        api_csrf(&s, &h)?;
        let Json(_) = body.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        let key = command_key(&h)?;
        s.funding
            .mature_publisher(&a, 100)
            .await
            .map_err(ad_error)?;
        let credited = s
            .funding
            .release_publisher(&a, &key, 100)
            .await
            .map_err(ad_error)?;
        Ok::<_, ApiError>(secure(Json(json!({"credited_minor":credited.get()})), &[]))
    }
    .await;
    result.unwrap_or_else(error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClickBody {
    viewability_token: String,
}
pub(super) async fn click(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Json<ClickBody>, JsonRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, false).await?;
        api_csrf(&s, &h)?;
        let Json(b) = b.map_err(json_status)?;
        let id = s
            .catalog
            .click(&a, &b.viewability_token)
            .await
            .map_err(ad_error)?;
        let cookie = format!(
            "tor-ad-context={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=1800{}",
            b.viewability_token,
            if s.cookies.secure { "; Secure" } else { "" }
        );
        Ok::<_, ApiError>(secure(
            Json(json!({"book_id":id.to_string(),"url":format!("/books/{id}")})),
            &[cookie],
        ))
    }
    .await;
    result.unwrap_or_else(error)
}

fn cancel_reason() -> String {
    "Запрос отмены кампании".into()
}
