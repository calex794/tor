use super::*;
use crate::pages::{escape_html as esc, layout};
use axum::{
    extract::{rejection::FormRejection, Form, Path, State},
    response::Redirect,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_ads::domain::paid::{PublishDaily, PublishPackage, SlotPolicy};

pub(super) async fn page(State(s): State<Runtime>, h: HeaderMap) -> Response {
    let result=async{
  let a=actor(&s,&h,true).await?;let ((offers,packages),policies)=tokio::try_join!(s.catalog.admin_catalog(&a),s.catalog.policies(&a)).map_err(status)?;
  let (csrf,cookies)=s.cookies.csrf_for_page(&h);let hidden=format!("<input type=\"hidden\" name=\"csrf_token\" value=\"{}\">",esc(&csrf));
  let mut body=String::from("<div class=\"ads-console\"><h1>Платное продвижение книг</h1><p>Публикация тарифа создаёт неизменяемую редакцию. Оплаченные кампании сохраняют прежние цены и условия. Покупки, размещения и расчёт включаются отдельными настройками сервера.</p>");
  body.push_str("<h2>Текущие редакции тарифов</h2><ul>");
  for o in offers {write!(body,"<li>Дни: ID {}, редакция {}, {} — {} за день; условия {}; продажа: {}</li>",o.id,o.revision,esc(render::slot_label(&o.slot)),render::money(o.rate_minor),esc(&o.terms_version),if o.enabled{"включена"}else{"выключена"}).unwrap();}
  for p in packages {write!(body,"<li>Показы: ID {}, редакция {}, {} показов за {}; условия {}; продажа: {}</li>",p.id,p.revision,p.target_impressions,render::money(p.price_minor),esc(&p.terms_version),if p.enabled{"включена"}else{"выключена"}).unwrap();}
  body.push_str("</ul><h2>Тариф по дням</h2><p>При отмене возвращаются будущие полные дни по UTC; начавшийся день оплачивается целиком. Публикуйте только согласованные условия.</p>");
  write!(body,"<form action=\"/admin/promotion/paid/daily-offer\" method=\"post\">{hidden}{}
    <label>Место<select name=\"slot\">{}</select></label>{}{}{}<label>Правило отмены<select name=\"cancellation_policy\"><option value=\"unused_full_utc_days\">Возврат будущих полных дней UTC</option></select></label>{}{}{}<button>Опубликовать редакцию тарифа</button></form>",
    identity(),slots(),number("rate_minor","Цена дня, копейки",1),number("min_days","Минимум дней",1),number("max_days","Максимум дней",1),terms(),enabled(),daily_caps()).unwrap();
  write!(body,"<h2>Пакет показов</h2><form action=\"/admin/promotion/paid/chapter-package\" method=\"post\">{hidden}{}{}{}{}{}{}{}{}{}{}<button>Опубликовать редакцию пакета</button></form>",identity(),
    number("price_minor","Цена пакета, копейки",1),number("target_impressions","Количество показов",1),
    number("publisher_share_bps","Доля издателя, сотые доли процента (0–10000)",0),number("hold_seconds","Удержание дохода издателя, секунды",0),
    number("daily_cap","Лимит на читателя в сутки",1),number("hourly_cap","Лимит на читателя в час",1),
    number("session_cap","Лимит на сессию",1),terms(),enabled()).unwrap();
  body.push_str("<h2>Разрешённые рекламные места</h2><ul>");
  for (slot,rev,first,daily,chapter,max) in policies{write!(body,"<li>{}: редакция {rev}; платные сначала: {first}; дни: {daily}; показы: {chapter}; карточек: {max}</li>",esc(render::slot_label(&slot))).unwrap();}
  write!(body,"</ul><form action=\"/admin/promotion/paid/slot-policy\" method=\"post\">{hidden}<label>Место<select name=\"slot\">{}</select></label>{}{}{}{}{}<button>Сохранить размещение</button></form>",
    slots(),number("expected_revision","Текущая редакция (0 для нового места)",0),
    boolean("paid_first","Приоритет платной рекламы перед редакционным продвижением"),boolean("daily_enabled","Размещение по дням"),
    boolean("chapter_enabled","Пакеты показов (только после бесплатной главы)"),number("max_cards","Число карточек (1–4)",1)).unwrap();
  write!(body,"<h2>Отмена через поддержку</h2><p>Остаток возвращается рекламодателю. Ранее подтверждённые показы сначала проходят расчёт.</p><form method=\"post\" action=\"/admin/promotion/paid/chapter/0/cancel\" data-ads-admin-cancel>{hidden}<input type=\"hidden\" name=\"command_key\" value=\"{}\"><label>Продукт<select data-ads-product><option value=\"chapter\">Показы</option><option value=\"daily\">Дни</option></select></label><label>ID кампании<input data-ads-id type=\"number\" min=\"1\" required></label>{}<label>Причина<input name=\"reason\" maxlength=\"500\" required></label><button>Остановить и вернуть остаток</button></form></div>",
    tor_accounts::services::security::generate_session_token(),number("expected_revision","Редакция кампании",1)).unwrap();
  let html=layout("Платное продвижение","Управление рекламными тарифами",&body);
  Ok::<_,StatusCode>(secure(axum::response::Html(html.0.replace("</head>","<link rel=\"stylesheet\" href=\"/assets/paid-ads.css\"></head>")
    .replace("</body>","<script src=\"/assets/paid-ads.js\" defer></script></body>")),&cookies))
 }.await;
    result.unwrap_or_else(native_error)
}
fn daily_caps() -> String {
    [
        number("daily_cap", "Лимит размещений на читателя в сутки", 1),
        number("hourly_cap", "Лимит размещений на читателя в час", 1),
        number("session_cap", "Лимит размещений на сессию", 1),
        number(
            "min_interval_seconds",
            "Минимальная пауза между размещениями, секунды",
            0,
        ),
    ]
    .concat()
}
fn number(name: &str, label: &str, min: i32) -> String {
    format!("<label>{label}<input type=\"number\" name=\"{name}\" min=\"{min}\" step=\"1\" required></label>")
}
fn boolean(name: &str, label: &str) -> String {
    format!("<label>{label}<select name=\"{name}\"><option value=\"false\">Нет</option><option value=\"true\">Да</option></select></label>")
}
fn enabled() -> String {
    boolean("enabled", "Разрешить продажу этой редакции")
}
fn terms() -> &'static str {
    "<label>Версия согласованных условий<input name=\"terms_version\" maxlength=\"120\" required></label>"
}
fn identity() -> String {
    format!("<label>ID тарифа (пусто для нового)<input type=\"number\" min=\"1\" name=\"id\"></label>{}",number("expected_revision","Текущая редакция (0 для нового тарифа)",0))
}
fn slots() -> &'static str {
    "<option value=\"home_featured\">Главная</option><option value=\"book_detail_related\">Карточка книги</option><option value=\"chapter_end_free\">После бесплатной главы</option>"
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DailyForm {
    csrf_token: String,
    id: String,
    expected_revision: i64,
    slot: String,
    rate_minor: i64,
    min_days: i32,
    max_days: i32,
    cancellation_policy: String,
    terms_version: String,
    daily_cap: i32,
    hourly_cap: i32,
    session_cap: i32,
    min_interval_seconds: i32,
    enabled: bool,
}
pub(super) async fn daily(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Form<DailyForm>, FormRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, true).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(&s, &h, &b.csrf_token)?;
        s.catalog
            .publish_daily(
                &a,
                &PublishDaily {
                    id: optional_id(&b.id)?,
                    expected_revision: b.expected_revision,
                    slot: b.slot,
                    rate_minor: b.rate_minor,
                    min_days: b.min_days,
                    max_days: b.max_days,
                    daily_cap: b.daily_cap,
                    hourly_cap: b.hourly_cap,
                    session_cap: b.session_cap,
                    min_interval_seconds: b.min_interval_seconds,
                    cancellation_policy: b.cancellation_policy,
                    terms_version: b.terms_version,
                    enabled: b.enabled,
                },
            )
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(Redirect::to("/admin/promotion/paid"), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageForm {
    csrf_token: String,
    id: String,
    expected_revision: i64,
    price_minor: i64,
    target_impressions: i64,
    publisher_share_bps: i32,
    hold_seconds: i32,
    daily_cap: i32,
    hourly_cap: i32,
    session_cap: i32,
    terms_version: String,
    enabled: bool,
}
pub(super) async fn package(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Form<PackageForm>, FormRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, true).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(&s, &h, &b.csrf_token)?;
        s.catalog
            .publish_package(
                &a,
                &PublishPackage {
                    id: optional_id(&b.id)?,
                    expected_revision: b.expected_revision,
                    price_minor: b.price_minor,
                    target_impressions: b.target_impressions,
                    publisher_share_bps: b.publisher_share_bps,
                    hold_seconds: b.hold_seconds,
                    daily_cap: b.daily_cap,
                    hourly_cap: b.hourly_cap,
                    session_cap: b.session_cap,
                    terms_version: b.terms_version,
                    enabled: b.enabled,
                },
            )
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(Redirect::to("/admin/promotion/paid"), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SlotForm {
    csrf_token: String,
    slot: String,
    expected_revision: i64,
    paid_first: bool,
    daily_enabled: bool,
    chapter_enabled: bool,
    max_cards: i32,
}
pub(super) async fn slot_policy(
    State(s): State<Runtime>,
    h: HeaderMap,
    b: Result<Form<SlotForm>, FormRejection>,
) -> Response {
    let result = async {
        let a = actor(&s, &h, true).await?;
        let Form(b) = b.map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
        csrf(&s, &h, &b.csrf_token)?;
        s.catalog
            .slot_policy(
                &a,
                &SlotPolicy {
                    slot: b.slot,
                    expected_revision: b.expected_revision,
                    paid_first: b.paid_first,
                    daily_enabled: b.daily_enabled,
                    chapter_enabled: b.chapter_enabled,
                    max_cards: b.max_cards,
                },
            )
            .await
            .map_err(status)?;
        Ok::<_, StatusCode>(secure(Redirect::to("/admin/promotion/paid"), &[]))
    }
    .await;
    result.unwrap_or_else(native_error)
}
pub(super) async fn cancel(
    State(s): State<Runtime>,
    h: HeaderMap,
    Path((p, id)): Path<(String, i64)>,
    b: Result<Form<console::CancelForm>, FormRejection>,
) -> Response {
    console::cancel_form(&s, &h, &p, id, b, true).await
}
fn optional_id(s: &str) -> Result<Option<i64>, StatusCode> {
    if s.is_empty() {
        Ok(None)
    } else {
        s.parse()
            .ok()
            .filter(|v| *v > 0)
            .map(Some)
            .ok_or(StatusCode::UNPROCESSABLE_ENTITY)
    }
}
