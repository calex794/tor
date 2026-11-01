//! Private D-06 browser flow. Never mount unless explicitly enabled.
//! Quote is read-only; the finance writer repeats all eligibility checks.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{extract::{Form, State}, http::{HeaderMap, StatusCode}, response::{Redirect, Response}};
use std::{collections::HashMap,fmt::Write};
use sqlx::Row;
use tor_accounts::services::security::generate_session_token;
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::{
        direct_swaps::{PgDirectSwaps, SwapError},
        refund_preflight::{DirectRefundGate, PgDirectRefundPreflight},
        swap_intent::DirectSwapIntent,
    },
};

fn rub(minor:i64)->String { format!("{},{:02} ₽",minor/100,minor%100) }
fn shell(content:&str)->axum::response::Html<String> {
    pages::layout("Обмен платных глав","Обмен неоткрытой купленной главы на другую главу того же автора.",
        &format!("<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card\"><p class=\"eyebrow\">Кабинет читателя</p><h1>Обмен главы</h1>{content}<p><a href=\"/me/purchases\">Мои покупки</a></p></div></div></section>"))
}
fn error(status:StatusCode,message:&str)->Response {
    private_response((status,shell(&format!("<p role=\"alert\">{}</p>",pages::escape_html(message)))),&[])
}
fn valid_id(value:&str)->Option<i64> {
    if value.is_empty()||value.len()>19||!value.bytes().all(|b|b.is_ascii_digit()){return None}
    value.parse::<i64>().ok().filter(|x|*x>0)
}
fn exact_fields(form:&HashMap<String,String>,step:&str)->bool {
    let names: &[&str]=if step=="preview" { &["csrf_token","step","choice"] }
        else { &["csrf_token","step","source","target","version","source_price","target_price","command_key"] };
    form.len()==names.len()&&names.iter().all(|name|form.contains_key(*name))
}
fn parse_choice(value:&str)->Option<(i64,i64)> {
    let (source,target)=value.split_once(':')?;
    Some((valid_id(source)?,valid_id(target)?))
}
fn intent(actor:i64,form:&HashMap<String,String>)->Option<DirectSwapIntent> {
    Some(DirectSwapIntent{
        buyer_id:actor,
        source_purchase_id:valid_id(form.get("source")?)?,
        target_chapter_id:valid_id(form.get("target")?)?,
        target_version_id:valid_id(form.get("version")?)?,
        source_price_minor:valid_id(form.get("source_price")?)?,
        target_price_minor:valid_id(form.get("target_price")?)?,
    })
}
fn message(error:SwapError)->(StatusCode,&'static str) {
    match error {
        SwapError::Invalid=>(StatusCode::UNPROCESSABLE_ENTITY,"Некорректное подтверждение обмена."),
        SwapError::Conflict=>(StatusCode::CONFLICT,"Этот ключ уже использован для другого обмена."),
        SwapError::NotFound=>(StatusCode::NOT_FOUND,"Исходная покупка не найдена."),
        SwapError::Changed=>(StatusCode::CONFLICT,"Цена или редакция изменилась. Повторите выбор."),
        SwapError::AlreadyOwned=>(StatusCode::CONFLICT,"Эта глава уже куплена."),
        SwapError::Opened=>(StatusCode::CONFLICT,"Исходная глава уже открывалась."),
        SwapError::Refunding=>(StatusCode::CONFLICT,"По покупке уже рассматривается возврат."),
        SwapError::Expired=>(StatusCode::CONFLICT,"Срок обмена истёк."),
        SwapError::NoPolicy|SwapError::Ineligible=>(StatusCode::CONFLICT,"Эта покупка недоступна для обмена."),
        SwapError::Insufficient=>(StatusCode::CONFLICT,"Недостаточно средств на внутреннем счёте."),
        SwapError::Unavailable=>(StatusCode::SERVICE_UNAVAILABLE,"Обмен временно недоступен. Повторите запрос позже."),
    }
}
fn eligible(gate:DirectRefundGate)->bool {
    gate==DirectRefundGate::FirstGrantNotSeenInTrackedPurchase
}
pub(super) async fn page(State(state):State<AuthState>,headers:HeaderMap)->Response {
    if !state.swap_browser_enabled {return deny(StatusCode::NOT_FOUND)}
    let actor=match session_user(&state,&headers).await {
        Ok(Some(id))=>id,Ok(None)=>return private_response(Redirect::to("/login"),&[]),
        Err(e)=>return deny(e),
    };
    // Candidate query bounded before calling quote preflight; it contains no
    // purchased content, only buyer-owned IDs and published chapter titles.
    let rows=sqlx::query(
        "SELECT src.id AS source_id, target.id AS target_id,
                source_book.title AS source_book,source_ch.title AS source_chapter,
                target_book.title AS target_book,target.title AS target_chapter
         FROM chapter_purchase_facts src
         JOIN chapters source_ch ON source_ch.id=src.chapter_id
         JOIN books source_book ON source_book.id=source_ch.book_id
         JOIN books target_book ON target_book.owner_user_id=source_book.owner_user_id
             AND target_book.id<>source_book.id
         JOIN chapters target ON target.book_id=target_book.id
         JOIN chapter_publications publication ON publication.chapter_id=target.id
         WHERE src.user_id=$1 AND src.first_grant_tracking_version='c06-first-issue-v1'
           AND src.refund_policy_version IS NOT NULL AND src.refund_deadline_at>now()
           AND NOT EXISTS (SELECT 1 FROM chapter_purchase_revocations rev WHERE rev.purchase_id=src.id)
           AND NOT EXISTS (SELECT 1 FROM chapter_purchase_first_exposures e WHERE e.purchase_id=src.id)
           AND target.access_level='paid' AND target.visibility='public'
           AND target.price_amount>0 AND NOT target.admin_hold AND NOT target.is_deleted
           AND (target.reader_released_at IS NULL OR target.reader_released_at<=now())
           AND target_book.visibility='public' AND NOT target_book.admin_hold
           AND NOT target_book.is_deleted
           AND source_book.owner_user_id IS NOT NULL AND source_book.owner_user_id<>$1
           AND NOT EXISTS (SELECT 1 FROM library_purchased_chapters l
                           WHERE l.user_id=$1 AND l.chapter_id=target.id)
         ORDER BY src.id DESC,target.id DESC LIMIT 60",
    ).bind(actor).fetch_all(state.accounts.pool()).await;
    let rows=match rows {Ok(rows)=>rows,Err(_)=>return deny(StatusCode::SERVICE_UNAVAILABLE)};
    let preflight=PgDirectRefundPreflight::new(state.accounts.pool().clone());
    let mut options=String::new();
    for row in rows {
        let source_id:i64=match row.try_get("source_id"){Ok(v)=>v,Err(_)=>return deny(StatusCode::SERVICE_UNAVAILABLE)};
        let target_id:i64=match row.try_get("target_id"){Ok(v)=>v,Err(_)=>return deny(StatusCode::SERVICE_UNAVAILABLE)};
        let quote=match preflight.swap_quote(actor,source_id,target_id).await {
            Ok(quote) if eligible(quote.gate)=>quote,
            _=>continue,
        };
        let mut title=String::new();
        for col in ["source_book","source_chapter","target_book","target_chapter"] {
            if !title.is_empty(){title.push_str(" — ")}
            let value:String=match row.try_get(col){Ok(v)=>v,Err(_)=>return deny(StatusCode::SERVICE_UNAVAILABLE)};
            title.push_str(&pages::escape_html(&value));
        }
        let difference=if quote.delta_minor>=0 {
            format!("доплата {}",rub(quote.delta_minor))
        }else{
            format!("возврат разницы {}",rub(-quote.delta_minor))
        };
        write!(options,"<option value=\"{}:{}\">{title} ({difference})</option>",source_id,target_id).expect("html");
    }
    let (csrf,cookies)=state.cookies.csrf_for_page(&headers);
    let body=if options.is_empty() {
        "<p>Сейчас нет доступных обменов. Обмен возможен только для неоткрытых покупок с действующим сроком и утверждёнными условиями.</p>".to_owned()
    }else{
        format!("<p>Выберите неоткрытую купленную главу и платную главу из другой книги того же автора.</p>
           <form method=\"post\" action=\"/me/refunds/swap\"><input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
           <input type=\"hidden\" name=\"step\" value=\"preview\">
           <label for=\"choice\">Вариант обмена</label><select id=\"choice\" name=\"choice\" required>{options}</select>
           <button class=\"button\" type=\"submit\">Проверить обмен и сумму</button></form>",
           pages::escape_html(&csrf))
    };
    private_response(shell(&body),&cookies)
}
pub(super) async fn submit(
    State(state):State<AuthState>,headers:HeaderMap,Form(form):Form<HashMap<String,String>>,
)->Response {
    if !state.swap_browser_enabled {return deny(StatusCode::NOT_FOUND)}
    let actor=match session_user(&state,&headers).await {
        Ok(Some(id))=>id,Ok(None)=>return deny(StatusCode::UNAUTHORIZED),Err(e)=>return deny(e),
    };
    if !state.cookies.same_origin(&headers) ||
        !form.get("csrf_token").is_some_and(|v|state.cookies.valid_csrf(&headers,v)) {
        return deny(StatusCode::FORBIDDEN);
    }
    let step=form.get("step").map(String::as_str).unwrap_or_default();
    if !exact_fields(&form,step){return deny(StatusCode::UNPROCESSABLE_ENTITY)}
    if step=="preview" {
        let (source,target)=match form.get("choice").and_then(|v|parse_choice(v)){
            Some(pair)=>pair,None=>return deny(StatusCode::UNPROCESSABLE_ENTITY),
        };
        let quote=match PgDirectRefundPreflight::new(state.accounts.pool().clone())
            .swap_quote(actor,source,target).await {
            Ok(q) if eligible(q.gate)=>q,
            Ok(_)=>return error(StatusCode::CONFLICT,"Исходная покупка не подлежит обмену."),
            Err(_)=>return error(StatusCode::CONFLICT,"Вариант обмена недоступен. Обновите страницу."),
        };
        let delta=if quote.delta_minor>0 {
            format!("Доплата {}",rub(quote.delta_minor))
        }else if quote.delta_minor<0{
            format!("На ваш внутренний счёт вернётся {}",rub(-quote.delta_minor))
        }else{"Без доплаты".to_owned()};
        let html=format!("<p>Стоимость старой главы: {}. Стоимость новой: {}.</p>
          <p role=\"status\">{delta}</p><p>Старый доступ будет отозван после подтверждения. Деньги и права изменятся только целиком.</p>
          <form method=\"post\" action=\"/me/refunds/swap\"><input type=\"hidden\" name=\"step\" value=\"commit\">
          <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
          <input type=\"hidden\" name=\"source\" value=\"{}\"><input type=\"hidden\" name=\"target\" value=\"{}\">
          <input type=\"hidden\" name=\"version\" value=\"{}\">
          <input type=\"hidden\" name=\"source_price\" value=\"{}\"><input type=\"hidden\" name=\"target_price\" value=\"{}\">
          <input type=\"hidden\" name=\"command_key\" value=\"{}\">
          <button class=\"button\" type=\"submit\">Подтвердить обмен</button></form><p><a href=\"/me/refunds/swap\">Отменить</a></p>",
          rub(quote.source_price_minor),rub(quote.target_price_minor),
          pages::escape_html(form.get("csrf_token").map(String::as_str).unwrap_or_default()),
          quote.source_purchase_id,quote.target_chapter_id,quote.target_version_id,
          quote.source_price_minor,quote.target_price_minor,generate_session_token());
        return private_response(shell(&html),&[])
    }
    if step!="commit" {return deny(StatusCode::UNPROCESSABLE_ENTITY)}
    let intent=match intent(actor,&form) {Some(i)=>i,None=>return deny(StatusCode::UNPROCESSABLE_ENTITY)};
    let key=match form.get("command_key").and_then(|s|IdempotencyKey::parse(s).ok()){
        Some(key)=>key,None=>return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    let receipt=match PgDirectSwaps::new(state.accounts.pool().clone()).swap_once(intent,&key).await {
        Ok(r)=>r,Err(e)=>{let (status,msg)=message(e);return error(status,msg)}
    };
    let detail=if receipt.delta_minor>0 {format!("Доплачено {}.",rub(receipt.delta_minor))}
        else if receipt.delta_minor<0{format!("Возвращено {}.",rub(-receipt.delta_minor))}
        else{"Обмен без доплаты.".to_owned()};
    let html=format!("<p role=\"status\">Глава обменена. {detail} Остаток на внутреннем счёте: {}.</p>
        <p>{}</p><p><a class=\"button\" href=\"/me/purchases\">Перейти к купленным главам</a></p>",
        rub(receipt.balance_after),if receipt.replayed{"Повторный запрос не списал деньги."}else{""});
    private_response(shell(&html),&[])
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn d_06_swap_browser_fails_closed_on_overposted_fields() {
        let mut v=HashMap::from([("step".into(),"preview".into()),("choice".into(),"1:2".into()),
            ("csrf_token".into(),"csrf".into())]);
        assert!(exact_fields(&v,"preview"));v.insert("user_id".into(),"8".into());
        assert!(!exact_fields(&v,"preview"));assert_eq!(parse_choice("1:0"),None);
        assert_eq!(parse_choice("1:2:3"),None);
    }
    #[test] fn d_06_swap_browser_accepts_exact_actor_bound_intent() {
        let form=HashMap::from([("source".into(),"1".into()),("target".into(),"2".into()),
            ("version".into(),"3".into()),("source_price".into(),"100".into()),("target_price".into(),"120".into())]);
        let i=intent(7,&form).unwrap();assert_eq!((i.buyer_id,i.source_purchase_id,i.target_chapter_id),(7,1,2));
        assert!(i.digest().is_ok());
    }
}
