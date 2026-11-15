use super::{display::render_cards, PATH};
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_ads::domain::editorial::{Campaign, CampaignPage, EditorialCard};
use tor_contracts::exposure::PromotionSlot;

fn slot_label(slot: PromotionSlot) -> &'static str {
    match slot {
        PromotionSlot::HomeFeatured => "Главная",
        PromotionSlot::BookDetailRelated => "Карточка книги",
        PromotionSlot::ChapterEndFree => "После бесплатной главы",
    }
}
fn hidden(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
        escape_html(value)
    )
}
fn security(csrf: &str) -> String {
    format!(
        "{}{}",
        hidden("csrf_token", csrf),
        hidden("command_key", &generate_session_token())
    )
}
fn dates(start: &str, end: &str, priority: i16) -> String {
    format!("<label>Начало, UTC RFC3339<input name=\"starts_at\" value=\"{}\" placeholder=\"2026-11-11T00:00:00Z\" required maxlength=\"40\"></label><label>Окончание, UTC RFC3339<input name=\"ends_at\" value=\"{}\" placeholder=\"2026-11-18T00:00:00Z\" required maxlength=\"40\"></label><label>Приоритет (0–100)<input type=\"number\" name=\"priority\" min=\"0\" max=\"100\" value=\"{priority}\" required></label>",escape_html(start),escape_html(end))
}
fn campaign(c: &Campaign, csrf: &str) -> String {
    let status = match c.status.as_str() {
        "draft" => "Черновик",
        "active" => "Включено в заданном интервале",
        "paused" => "Приостановлено",
        "ended" => "Завершено",
        _ => "Неизвестно",
    };
    let availability = if c.discoverable {
        "Доступность и возраст проверяются для каждого зрителя."
    } else {
        "Книга сейчас недоступна для продвижения; выдача остановлена."
    };
    let mut body=format!("<article class=\"book-section-card\" data-editorial-management=\"{}\"><h2>{}</h2><p>{} · {} · Приоритет {}</p><p>С {} до {}. {availability}</p><p><a href=\"/admin/audience/exposure?source=editorial&amp;campaign={}&amp;days=7\">Показы и переходы</a> · <a href=\"/admin/audience/reading?days=7\">Подтверждённое чтение книг</a></p>",c.id,escape_html(&c.title),slot_label(c.slot),status,c.priority,escape_html(&c.starts_at),escape_html(&c.ends_at),c.id);
    if c.status != "ended" {
        let options = if c.status == "active" {
            "<option value=\"pause\">Приостановить</option>"
        } else {
            "<option value=\"activate\">Включить</option>"
        };
        write!(body,"<form class=\"auth-form author-form\" method=\"post\" action=\"{PATH}\" data-editorial-state=\"{}\">{}{}{}<label>Действие<select name=\"action\">{options}<option value=\"end\">Завершить навсегда</option></select></label><button class=\"button\" type=\"submit\">Применить</button></form>",c.id,security(csrf),hidden("campaign_id",&c.id.to_string()),hidden("expected_revision",&c.revision.to_string())).unwrap();
        if c.status != "active" {
            write!(body,"<details><summary>Изменить расписание и приоритет</summary><form class=\"auth-form author-form\" method=\"post\" action=\"{PATH}\">{}{}{}{}{}{}{}<button class=\"button\" type=\"submit\">Сохранить изменения</button></form></details>",security(csrf),hidden("action","update"),hidden("campaign_id",&c.id.to_string()),hidden("expected_revision",&c.revision.to_string()),hidden("book_id",&c.book_id.to_string()),hidden("slot",c.slot.as_str()),dates(&c.starts_at,&c.ends_at,c.priority)).unwrap();
        }
    }
    if c.discoverable {
        let preview = render_cards(
            &[EditorialCard {
                campaign_id: c.id,
                revision: c.revision,
                book_id: c.book_id,
                title: c.title.clone(),
                slot: c.slot,
            }],
            None,
        );
        write!(body,"<details data-editorial-preview><summary>Предпросмотр карточки</summary><p>Предпросмотр не входит в статистику.</p>{preview}</details>").unwrap();
    }
    body.push_str("</article>");
    body
}
pub(super) fn page(
    report: &CampaignPage,
    csrf: &str,
    enabled: bool,
    changed: Option<i64>,
) -> Html<String> {
    let mut content=String::from("<section class=\"section\"><div class=\"container\"><nav><a href=\"/me\">Кабинет</a> · <a href=\"/admin/audience\">Аудитория</a></nav><h1>Редакционное продвижение</h1><p>Продвигайте собственные книги на главной, в карточках книг и после бесплатных глав. Размещения не связаны с оплатой.</p>");
    if !enabled {
        content.push_str("<p role=\"status\">Публичные размещения сейчас отключены. Назначения можно подготовить и проверить.</p>");
    }
    if let Some(id) = changed {
        write!(
            content,
            "<p role=\"status\">Изменения назначения №{id} сохранены.</p>"
        )
        .unwrap();
    }
    content.push_str("<p>Показы и переходы измеряются отдельно от чтения. Чтение книги за период не доказывает результат конкретного размещения.</p><section class=\"book-section-card\"><h2>Новое назначение</h2>");
    write!(content,"<form class=\"auth-form author-form\" method=\"post\" action=\"{PATH}\" data-editorial-create>{}{}{}<label>ID книги<input type=\"text\" inputmode=\"numeric\" name=\"book_id\" pattern=\"[1-9][0-9]*\" required></label><p>ID находится в адресе карточки книги: /books/123. После сохранения доступен предпросмотр; новый черновик ещё не показывается.</p><label>Место<select name=\"slot\">",security(csrf),hidden("action","create"),hidden("expected_revision","0")).unwrap();
    for slot in PromotionSlot::ALL {
        write!(
            content,
            "<option value=\"{}\">{}</option>",
            slot.as_str(),
            slot_label(slot)
        )
        .unwrap();
    }
    write!(
        content,
        "</select></label>{}<button class=\"button\" type=\"submit\">Создать черновик</button></form></section>",
        dates("", "", 0)
    )
    .unwrap();
    if report.items.is_empty() {
        content.push_str("<p>Назначений пока нет.</p>");
    }
    for item in &report.items {
        content.push_str(&campaign(item, csrf));
    }
    if let Some(after) = report.next_cursor {
        write!(
            content,
            "<p><a href=\"{PATH}?after={after}\">Следующие назначения</a></p>"
        )
        .unwrap();
    }
    content.push_str("</div></section>");
    layout(
        "Редакционное продвижение",
        "Управление редакционными размещениями книг.",
        &content,
    )
}
