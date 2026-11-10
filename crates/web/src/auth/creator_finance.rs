//! S038: actor-scoped creator-finance statement and guarded pending intent.
//! No reader-wallet cash-out, bank details, or external payout are implemented.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{DefaultBodyLimit, Form, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::creator_finance::{
        CreatorEarningLine, CreatorFinanceError, CreatorFinanceTotals, CreatorPayoutRequest,
        CreatorRefundLine, PgCreatorFinance,
    },
};

#[derive(Deserialize, Default)]
struct FinanceQuery {
    tab: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinanceTab {
    Stats,
    Refunds,
    Payouts,
}

impl FinanceTab {
    fn parse(tab: Option<&str>) -> Self {
        match tab {
            Some("refunds") => Self::Refunds,
            Some("payouts") => Self::Payouts,
            _ => Self::Stats,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestForm {
    csrf_token: String,
    intent_key: String,
    amount_rub: String,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route(
        "/me/finance",
        get(page).post(submit).layer(DefaultBodyLimit::max(4096)),
    )
}

/// Requires an explicit operator opt-in. Default production and CI is OFF.
/// A pending request is not payment authorization or external settlement.
fn reservations_enabled() -> bool {
    std::env::var("TOR_CREATOR_PAYOUT_REQUESTS_ENABLED").as_deref() == Ok("1")
}

fn rub(minor: i64) -> String {
    format!("{}.{:02} RUB", minor / 100, minor % 100)
}

fn rub_to_minor(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > 16 {
        return None;
    }
    let (whole, fraction) = match trimmed.split_once(['.', ',']) {
        Some((whole, fraction)) => (whole, fraction),
        None => (trimmed, "00"),
    };
    if whole.is_empty()
        || whole.len() > 12
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.is_empty()
        || fraction.len() > 2
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: i64 = whole.parse().ok()?;
    let fraction: i64 = if fraction.len() == 1 {
        fraction.parse::<i64>().ok()?.checked_mul(10)?
    } else {
        fraction.parse().ok()?
    };
    let minor = whole.checked_mul(100)?.checked_add(fraction)?;
    (1..=1_000_000_000_000).contains(&minor).then_some(minor)
}

fn noindex(mut response: Response) -> Response {
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow, noarchive"),
    );
    response
}

fn shell(content: &str) -> Html<String> {
    pages::layout(
        "Финансы автора",
        "Личный учёт подтверждённых авторских начислений.",
        &format!(
            "<section class=\"section auth-section\"><div class=\"container\">\
             <div class=\"auth-card\"><p class=\"eyebrow\">Личные финансы</p>\
             <h1>Финансы автора</h1>{content}\
             <p><a href=\"/me\">Личный кабинет</a> ·\
             <a href=\"/me/books\">Книги автора</a></p></div></div></section>"
        ),
    )
}

fn render(
    totals: &CreatorFinanceTotals,
    earnings: &[CreatorEarningLine],
    refunds: &[CreatorRefundLine],
    history: &[CreatorPayoutRequest],
    csrf: &str,
    allow_requests: bool,
    tab: FinanceTab,
) -> Html<String> {
    let mut rows = String::new();
    for row in history {
        let state = match row.state.as_str() {
            "pending" => "Ожидает решения — деньги не перечислены",
            "rejected" => "Отклонена — резерв освобождён",
            "posted" => "Внутренняя проводка создана — банковский перевод не подтверждён",
            _ => "Статус не подтверждён",
        };
        write!(
            rows,
            "<li>Заявка №{} от {} UTC: {} — {state}</li>",
            row.id,
            pages::escape_html(&row.created_utc),
            rub(row.amount_minor)
        )
        .expect("bounded history");
    }
    if rows.is_empty() {
        rows.push_str("<li>Заявок пока нет.</li>");
    }
    let form = if allow_requests && totals.available_minor > 0 {
        let key = generate_session_token();
        format!(
            "<form method=\"post\" action=\"/me/finance\">\
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
             <input type=\"hidden\" name=\"intent_key\" value=\"{}\">\
             <label for=\"payout-rub\">Сумма резерва, RUB (например, 100.50)</label>\
             <input id=\"payout-rub\" name=\"amount_rub\" type=\"text\"\
               inputmode=\"decimal\" maxlength=\"16\" required>\
             <button type=\"submit\">Создать заявку на резерв</button></form>",
            pages::escape_html(csrf),
            pages::escape_html(&key)
        )
    } else {
        "<p>Приём заявок на резервирование сейчас выключен.\
         Это не блокирует просмотр уже начисленного дохода.</p>"
            .to_owned()
    };
    // In NEwBOO the finance area has three tabs. Render only the actor-bound,
    // bounded read model selected by the visitor. Neither a book title nor
    // another user's share or any buyer identity is projected.
    let mut details = String::new();
    match tab {
        FinanceTab::Stats => {
            details.push_str(&format!("<h2>Резервирование</h2>{form}"));
            details.push_str("<h2>Последние начисления</h2><ul>");
            for line in earnings {
                let status = match line.status.as_str() {
                    "mature" => "Созрело",
                    "immature" => "Удержание до созревания",
                    "refund_hold" => "Ожидается решение по возврату",
                    "reversed" => "Подтверждён возврат",
                    "swapped" => "Начисление отменено обменом главы",
                    _ => "Статус не подтверждён",
                };
                write!(
                    details,
                    "<li>Покупка №{} · доля {}% · начислено {} · возвращено {} \
                     · {} UTC · {status}</li>",
                    line.purchase_id,
                    line.share_percent,
                    rub(line.amount_minor),
                    rub(line.reversed_minor),
                    pages::escape_html(&line.earned_utc),
                )
                .expect("bounded earning rows");
            }
            if earnings.is_empty() {
                details.push_str("<li>Подтверждённых начислений пока нет.</li>");
            }
            details.push_str("</ul><p>Показаны последние 50 начислений.</p>");
        }
        FinanceTab::Refunds => {
            details.push_str("<h2>Возвраты авторских начислений</h2><ul>");
            for line in refunds {
                write!(
                    details,
                    "<li>Покупка №{} · удержано {} · {} UTC</li>",
                    line.purchase_id,
                    rub(line.amount_minor),
                    pages::escape_html(&line.reversed_utc)
                )
                .expect("bounded refund rows");
            }
            if refunds.is_empty() {
                details.push_str("<li>Подтверждённых возвратов пока нет.</li>");
            }
            details.push_str("</ul><p>Показаны последние 50 возвратов.</p>");
        }
        FinanceTab::Payouts => {
            details.push_str(&format!(
                "<h2>История заявок на резерв</h2><ul>{rows}</ul>\
                 <p>Показаны последние 30 заявок. Ни одна заявка не является\
                 подтверждением банковской выплаты.</p>"
            ));
        }
    }
    shell(&format!(
        "<p>Единый расчёт по модели NEwBOO: баланс кошелька и авторские\
         начисления учитываются вместе, независимо от источника пополнения.\
         Незрелые начисления, открытые возвраты и заявки удерживаются.</p>\
         <dl class=\"finance-breakdown\">\
         <dt>Баланс кошелька</dt><dd>{}</dd>\
         <dt>Общий расчётный баланс</dt><dd>{}</dd>\
         <dt>Начислено до возвратов</dt><dd>{}</dd>\
         <dt>Подтверждённые возвраты</dt><dd>{}</dd>\
         <dt>Ещё не созрело</dt><dd>{}</dd>\
         <dt>Созревшие начисления</dt><dd>{}</dd>\
         <dt>Удержано под незавершённые возвраты</dt><dd>{}</dd>\
         <dt>Зарезервировано под заявки</dt><dd>{}</dd>\
         <dt>Потенциально доступно к резерву</dt><dd><strong>{}</strong></dd>\
         </dl>\
         <p>Потенциально доступная сумма — внутренний расчёт, не обещание выплаты.\
         До подтверждения договорных условий и платёжного провайдера\
         реальные перечисления отключены.</p>\
         <nav aria-label=\"Разделы финансов\">\
         <a href=\"/me/finance?tab=stats\">Начисления</a> · \
         <a href=\"/me/finance?tab=refunds\">Возвраты</a> · \
         <a href=\"/me/finance?tab=payouts\">Заявки</a></nav>\
         {details}",
        rub(totals.wallet_balance_minor),
        rub(totals.combined_balance_minor),
        rub(totals.gross_minor),
        rub(totals.reversed_minor),
        rub(totals.immature_minor),
        rub(totals.mature_minor),
        rub(totals.open_refund_hold_minor),
        rub(totals.reserved_minor),
        rub(totals.available_minor),
    ))
}

fn request_error(error: CreatorFinanceError) -> StatusCode {
    match error {
        CreatorFinanceError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        CreatorFinanceError::Forbidden => StatusCode::FORBIDDEN,
        CreatorFinanceError::Conflict => StatusCode::CONFLICT,
        CreatorFinanceError::Insufficient => StatusCode::CONFLICT,
        CreatorFinanceError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        CreatorFinanceError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn page(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<FinanceQuery>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return noindex(private_response(Redirect::to("/login"), &[])),
        Err(status) => return deny(status),
    };
    let repo = PgCreatorFinance::new(state.accounts.pool().clone());
    let totals = match repo.totals(actor).await {
        Ok(totals) => totals,
        Err(e) => return deny(request_error(e)),
    };
    let tab = FinanceTab::parse(query.tab.as_deref());
    let earnings = if tab == FinanceTab::Stats {
        match repo.recent_earnings(actor).await {
            Ok(rows) => rows,
            Err(e) => return deny(request_error(e)),
        }
    } else {
        Vec::new()
    };
    let refunds = if tab == FinanceTab::Refunds {
        match repo.recent_refunds(actor).await {
            Ok(rows) => rows,
            Err(e) => return deny(request_error(e)),
        }
    } else {
        Vec::new()
    };
    let history = if tab == FinanceTab::Payouts {
        match repo.history(actor).await {
            Ok(rows) => rows,
            Err(e) => return deny(request_error(e)),
        }
    } else {
        Vec::new()
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    noindex(private_response(
        render(
            &totals,
            &earnings,
            &refunds,
            &history,
            &csrf,
            reservations_enabled(),
            tab,
        ),
        &cookies,
    ))
}

async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<RequestForm>,
) -> Response {
    if !reservations_enabled() {
        return deny(StatusCode::NOT_FOUND);
    }
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let Some(amount_minor) = rub_to_minor(&form.amount_rub) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let Ok(key) = IdempotencyKey::parse(&form.intent_key) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    match PgCreatorFinance::new(state.accounts.pool().clone())
        .reserve_once(actor, amount_minor, &key)
        .await
    {
        Ok(_) => noindex(private_response(Redirect::to("/me/finance"), &[])),
        Err(error) => deny(request_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        render, rub_to_minor, CreatorEarningLine, CreatorFinanceTotals, CreatorRefundLine,
        FinanceTab,
    };

    #[test]
    fn s038_strict_minor_units_without_floating_rounding() {
        assert_eq!(rub_to_minor("10"), Some(1000));
        assert_eq!(rub_to_minor("10.01"), Some(1001));
        assert_eq!(rub_to_minor("10,5"), Some(1050));
        for raw in ["0", "-1", "1e2", "1.009", "1..00", "", "NaN"] {
            assert_eq!(rub_to_minor(raw), None);
        }
    }

    #[test]
    fn s038_newboo_tabs_show_only_owner_financial_line_facts() {
        let totals = CreatorFinanceTotals {
            wallet_balance_minor: 0,
            combined_balance_minor: 150,
            gross_minor: 200,
            reversed_minor: 50,
            immature_minor: 0,
            mature_minor: 150,
            open_refund_hold_minor: 0,
            reserved_minor: 0,
            available_minor: 150,
        };
        let earned = CreatorEarningLine {
            purchase_id: 38,
            amount_minor: 200,
            share_percent: 60,
            reversed_minor: 50,
            status: "reversed".into(),
            earned_utc: "2026-11-09 10:00".into(),
        };
        let refund = CreatorRefundLine {
            purchase_id: 38,
            amount_minor: 50,
            reversed_utc: "2026-11-09 11:00".into(),
        };
        let stats = render(
            &totals,
            &[earned],
            &[],
            &[],
            "csrf",
            false,
            FinanceTab::Stats,
        )
        .0;
        assert!(stats.contains("Покупка №38"));
        assert!(stats.contains("доля 60%"));
        assert!(stats.contains("Подтверждён возврат"));
        let swapped = CreatorEarningLine {
            purchase_id: 39,
            amount_minor: 200,
            share_percent: 60,
            reversed_minor: 200,
            status: "swapped".into(),
            earned_utc: "2026-11-09 10:00".into(),
        };
        let swap_html = render(
            &totals,
            &[swapped],
            &[],
            &[],
            "csrf",
            false,
            FinanceTab::Stats,
        )
        .0;
        assert!(swap_html.contains("Начисление отменено обменом главы"));
        assert!(stats.contains("Единый расчёт по модели NEwBOO"));
        assert!(stats.contains("Общий расчётный баланс"));
        assert!(stats.contains("href=\"/me/finance?tab=refunds\""));
        assert!(!stats.contains("username"));
        let refunds = render(
            &totals,
            &[],
            &[refund],
            &[],
            "csrf",
            false,
            FinanceTab::Refunds,
        )
        .0;
        assert!(refunds.contains("Покупка №38 · удержано 0.50 RUB"));
        assert!(!refunds.contains("доля 60%"));
        assert!(!refunds.contains("name=\"intent_key\""));
    }

    #[test]
    fn s038_default_finance_shell_cannot_claim_a_payout() {
        let totals = CreatorFinanceTotals {
            wallet_balance_minor: 0,
            combined_balance_minor: 200,
            gross_minor: 200,
            reversed_minor: 0,
            immature_minor: 200,
            mature_minor: 0,
            open_refund_hold_minor: 0,
            reserved_minor: 0,
            available_minor: 0,
        };
        let html = render(&totals, &[], &[], &[], "csrf", false, FinanceTab::Stats).0;
        assert!(html.contains("платёжного провайдера"));
        assert!(!html.contains("Создать заявку на резерв</button>"));
        assert!(!html.contains("name=\"intent_key\""));
        assert!(html.contains("0.00 RUB"));
        assert!(!html.contains("выплата выполнена"));
        let posted = CreatorPayoutRequest {
            id: 77,
            amount_minor: 200,
            state: "posted".into(),
            created_utc: "2026-11-10 00:00".into(),
        };
        let posted_html = render(
            &totals,
            &[],
            &[],
            &[posted],
            "csrf",
            false,
            FinanceTab::Payouts,
        )
        .0;
        assert!(posted_html.contains("Внутренняя проводка создана"));
        assert!(posted_html.contains("банковский перевод не подтверждён"));
        assert!(!posted_html.contains("выплата выполнена"));
    }
}
