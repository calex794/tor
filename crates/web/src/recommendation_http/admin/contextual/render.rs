//! Small bounded no-JavaScript operator page.
use super::*;
use crate::pages::escape_html;
use std::fmt::Write;
pub(super) fn page(status: Option<&Status>, csrf: &str, enabled: bool) -> String {
    let mut out = String::from(
        r#"<section class="section"><div class="container audience-books-width"><h1>Контекстные рекомендации</h1><p>Подбор по жанрам, тегам и ручным связям книг. История чтения и поведенческие оценки не используются.</p><p><a href="/me/recommendations/manage">Контекстные рекомендации: metadata_manual_v1</a></p>"#,
    );
    if !enabled {
        out.push_str(r#"<p data-testid="contextual-product-disabled">Выдача рекомендаций отключена продуктовым флагом. Сборка модели его не включает.</p>"#);
    }
    let Some(s) = status else {
        out.push_str(r#"<p role="alert" data-testid="contextual-unavailable">Данные временно недоступны. Команды не отправляются.</p></div></section>"#);
        return out;
    };
    write!(out,r#"<section class="book-section-card" data-testid="contextual-status" data-revision="{}"><p>{}</p><p>Активная версия: <span data-testid="contextual-active">{}</span></p><p>Предыдущая версия: <span data-testid="contextual-previous">{}</span></p>"#,s.revision,if s.held{"Контекстный подбор приостановлен: доступны новинки каталога."}else{"Контекстный подбор активен. Права и скрытия проверяются при каждой выдаче."},escape_html(s.active_generation.as_deref().unwrap_or("не выбрана")),escape_html(s.previous_generation.as_deref().unwrap_or("не выбрана"))).expect("String");
    form(
        &mut out,
        "refresh",
        "Собрать модель",
        None,
        s.revision,
        csrf,
    );
    if !s.held {
        form(
            &mut out,
            "hold",
            "Приостановить выдачу",
            None,
            s.revision,
            csrf,
        );
    }
    if s.previous_generation.is_some() {
        form(
            &mut out,
            "rollback",
            "Вернуть предыдущую версию",
            None,
            s.revision,
            csrf,
        );
    }
    out.push_str("</section><h2>Последние сборки</h2>");
    if s.generations.is_empty() {
        out.push_str("<p>Сборок новой модели пока нет.</p>");
    }
    for g in &s.generations {
        let is_active = s.active_generation.as_deref() == Some(g.id.as_str());
        let label = match (g.status.as_str(), is_active, s.held) {
            ("building", _, _) => "Собирается",
            ("validated", true, false) => "Активна",
            ("validated", true, true) => "Выбрана; выдача приостановлена",
            ("validated", false, _) => "Готова к активации",
            _ => "Отклонена",
        };
        write!(out,r#"<article class="book-section-card audience-book-row" data-testid="contextual-generation" data-generation="{}" data-status="{}"><h3>{}</h3><p>{}</p><p>{}</p>"#,escape_html(&g.id),escape_html(&g.status),escape_html(&g.id).replace("-", "-<wbr>"),label,escape_html(&g.created_at)).expect("String");
        if g.status == "validated" && (!is_active || s.held) {
            form(
                &mut out,
                "activate",
                "Включить эту версию",
                Some(&g.id),
                s.revision,
                csrf,
            );
        }
        if g.status == "building" {
            write!(
                out,
                "<p>Обработано до ID книги: {}. Это позиция, не количество книг.</p>",
                escape_html(&g.cursor_book_id)
            )
            .expect("String");
            form(
                &mut out,
                "cancel",
                "Отменить сборку",
                Some(&g.id),
                s.revision,
                csrf,
            );
        }
        out.push_str("</article>");
    }
    out.push_str("<p>Сборка не меняет активную версию. Включение и возврат выполняются отдельно. Старые поведенческие версии недоступны для возврата.</p></div></section>");
    out
}
fn form(
    out: &mut String,
    action: &str,
    label: &str,
    generation: Option<&str>,
    revision: i64,
    csrf: &str,
) {
    write!(out,r#"<form class="author-form" method="post" action="/me/recommendations/contextual" data-action="{action}"><input type="hidden" name="action" value="{action}"><input type="hidden" name="expected_revision" value="{revision}"><input type="hidden" name="csrf_token" value="{}">"#,escape_html(csrf)).expect("String");
    if let Some(id) = generation {
        write!(
            out,
            r#"<input type="hidden" name="generation" value="{}">"#,
            escape_html(id)
        )
        .expect("String");
    }
    write!(
        out,
        r#"<button class="button" type="submit">{label}</button></form>"#
    )
    .expect("String");
}
