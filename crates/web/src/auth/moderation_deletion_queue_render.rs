//! Private HTML for the independently moderated deletion queue.
//! Kept separate from the privileged decision handlers to enforce a bounded
//! module size as filtering and accessible UI grow.
use crate::pages;
use tor_accounts::repositories::moderation_books::ModerationDeletionRequest;

pub(super) fn render_deletion_queue(
    rows: &[ModerationDeletionRequest],
    csrf: &str,
    registration_count: i64,
    cover_count: i64,
    deletion_count: i64,
    search: Option<&str>,
    level: Option<&str>,
    wait: Option<&str>,
) -> String {
    let token = pages::escape_html(csrf);
    let search = pages::escape_html(search.unwrap_or(""));
    let level = level.unwrap_or("");
    let selected = |tier: &str| if level == tier { " selected" } else { "" };
    let level_all = selected("");
    let level_basic = selected("basic");
    let level_adult = selected("adult");
    let level_pro = selected("pro_adult");
    let level_legacy = selected("legacy");
    let wait = wait.unwrap_or("");
    let waiting_selected = |age: &str| if wait == age { " selected" } else { "" };
    let wait_any = waiting_selected("");
    let wait_new = waiting_selected("lt24");
    let wait_day = waiting_selected("gte24");
    let wait_three = waiting_selected("gte72");
    let mut cards = String::new();
    for item in rows {
        let title = pages::escape_html(&item.title);
        cards.push_str(&format!(
            r#"<article class="me-now-card"><h2>{title}</h2>
               <p>Книга #{book} · Владелец #{owner} · Заявка #{revision}</p>
               <p><a href="/moderation/books/{book}/history">История решений по книге</a></p>
               <details class="moderation-review-confirm" open>
                 <summary>Рассмотреть заявку на удаление книги #{book}</summary>
                 <p>Проверьте книгу и ревизию. Одобрение окончательно удалит
                    её из публичного каталога; сохранённые главы читателей останутся доступны.</p>
               <form method="post" action="/moderation/books/{book}/deletion">
                 <input type="hidden" name="csrf_token" value="{token}">
                 <input type="hidden" name="request_revision" value="{revision}">
                 <label for="deletion-reason-{book}">Причина отказа (обязательна для отказа)</label>
                 <input id="deletion-reason-{book}" name="reason" maxlength="2000">
                 <button type="submit" name="decision" value="approve">Одобрить и удалить</button>
                 <button type="submit" name="decision" value="reject">Отклонить удаление</button>
               </form></details></article>"#,
            book = item.book_id,
            owner = item.owner_user_id,
            revision = item.request_revision
        ));
    }
    if cards.is_empty() {
        cards.push_str("<p>Ожидающих решения заявок нет.</p>");
    }
    if rows.len() == 40 {
        if let Some(last) = rows.last() {
            cards.push_str(&format!(
                "<form method=\"get\" action=\"/moderation/books/deletions\">
                 <input type=\"hidden\" name=\"before\" value=\"{}\">
                 <input type=\"hidden\" name=\"q\" value=\"{search}\">
                 <input type=\"hidden\" name=\"level\" value=\"{level}\">
                 <input type=\"hidden\" name=\"wait\" value=\"{wait}\">
                 <button type=\"submit\">Следующая страница</button></form>",
                last.book_id
            ));
        }
    }
    pages::layout(
        "Модерация удаления книг",
        "Одобрение заявок на обычное удаление книги.",
        &format!(
            r#"<section class="me-page"><div class="container">
               <h1>Кабинет модератора</h1>
               <nav aria-label="Очереди модератора">
                 <a href="/moderation/books/registrations">Регистрация ({registration_count})</a>
                 <a href="/moderation/covers">Обложки ({cover_count})</a>
                 <a href="/moderation/books/deletions" aria-current="page">Удаление ({deletion_count})</a>
                 <a href="/moderation/books/control">Контроль</a>
               </nav>
              <p><a href="/moderation/books/reviews">Журнал рассмотренных заявок</a></p>
               <h2>Заявки на удаление книг</h2>
               <form method="get" action="/moderation/books/deletions">
                 <label for="deletion-search">Поиск по названию или ID книги</label>
                 <input id="deletion-search" name="q" maxlength="120" value="{search}">
                 <label for="deletion-level">Возрастная категория книги</label>
                 <select id="deletion-level" name="level">
                   <option value=""{level_all}>Все уровни</option>
                   <option value="basic"{level_basic}>Без ограничений</option>
                   <option value="adult"{level_adult}>18+</option>
                   <option value="pro_adult"{level_pro}>21+</option>
                   <option value="legacy"{level_legacy}>Не указан</option>
                 </select>
                 <label for="deletion-wait">Время ожидания заявки</label>
                 <select id="deletion-wait" name="wait">
                   <option value=""{wait_any}>Любое время</option>
                   <option value="lt24"{wait_new}>До 24 часов</option>
                   <option value="gte24"{wait_day}>От 24 часов</option>
                   <option value="gte72"{wait_three}>От 72 часов</option>
                 </select>
                 <button type="submit">Найти</button>
                 <a href="/moderation/books/deletions">Сбросить</a>
               </form>
               <p>Это решение не удаляет главы и не предоставляет платные права.
                  После одобрения книга удаляется из публичного доступа сразу.
                  Купленные и сохранённые главы остаются в личных библиотеках.</p>
               {cards}<p><a href="/moderation/books/control">Контроль и удержания книг</a></p>
               </div></section>"#
        ),
    )
    .0
}
