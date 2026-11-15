//! Private author book pages. No fake books and no client-supplied user IDs.
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::OwnedBook;
use tor_commerce::domain::layers::OwnedLayerSummary;

use crate::pages::{escape_html, private_account_layout as layout};

pub fn index(books: &[OwnedBook], csrf: &str) -> Html<String> {
    render_index(books, None, csrf)
}

/// The Layer section is visible only when the author management feature
/// is explicitly enabled; no purchase or subscription is implied.
pub fn index_with_layers(
    books: &[OwnedBook],
    layers: &[OwnedLayerSummary],
    csrf: &str,
) -> Html<String> {
    render_index(books, Some(layers), csrf)
}

fn render_index(
    books: &[OwnedBook],
    layers: Option<&[OwnedLayerSummary]>,
    csrf: &str,
) -> Html<String> {
    let mut list = String::new();
    if books.is_empty() {
        list.push_str(
            "<div class=\"author-workspace__empty\"><h3>У вас пока нет книг</h3>\
             <p>Создайте книгу, добавьте главы и подготовьте её к публикации.</p></div>",
        );
    } else {
        list.push_str("<ul class=\"author-workspace__books\">");
        for book in books {
            write!(
                &mut list,
                r#"<li class="author-workspace__book">
<h3><a class="author-book-link" href="/me/books/{id}">{title}</a></h3>
<a class="ui-button ui-button--ghost" href="/me/books/{id}">Управлять книгой</a>
<div class="author-workspace__actions">
<a class="text-link" href="/me/books/{id}/settings">Настройки</a>
<a class="text-link" href="/me/books/{id}/recommendations">Рекомендации</a>
</div>
<details class="author-workspace__more"><summary>Другие действия</summary>
<a href="/me/books/{id}/settings/delete">Удаление книги</a></details>
</li>"#,
                id = book.id,
                title = escape_html(&book.title),
            )
            .expect("write String");
        }
        list.push_str("</ul>");
        if books.len() == 100 {
            list.push_str("<p class=\"author-workspace__limit\">Показаны последние 100 книг.</p>");
        }
    }
    let layer_section = layers.map_or_else(String::new, |owned| {
        let mut section = String::from(
            "<section class=\"author-workspace__layers\" aria-labelledby=\"my-layers-heading\">\
             <h2 id=\"my-layers-heading\">Мои пакеты книг</h2>",
        );
        section.push_str(
            "<p><a class=\"button\" href=\"/me/layers/new\">Создать пакет</a> <a class=\"text-link\" href=\"/me/layers\">Все мои пакеты</a></p>",
        );
        if owned.is_empty() {
            section.push_str("<p class=\"empty-state\">Пакетов пока нет.</p>");
        } else {
            section.push_str("<ul class=\"author-book-list\">");
            for layer in owned {
                write!(
                    section,
                    "<li><a class=\"author-book-link\" href=\"/me/layers/{}/books\">{} — настроить пакет</a></li>",
                    layer.layer_id,
                    escape_html(&layer.title)
                )
                .expect("write String");
            }
            section.push_str("</ul>");
            if owned.len() == 100 {
                section.push_str("<p>Показаны последние 100 пакетов.</p>");
            }
        }
        section.push_str("</section>");
        section
    });
    let content = include_str!("../templates/author_books.html")
        .replace(
            "<!--ACCOUNT_NAVIGATION-->",
            &crate::pages::account_navigation("/me/books", csrf),
        )
        .replace("<!--BOOK_COUNT-->", &books.len().to_string())
        .replace("<!--BOOK_LIST-->", &list)
        .replace("<!--LAYER_SECTION-->", &layer_section);
    layout(
        "Мои книги",
        "Управление вашими книгами и главами.",
        &content,
    )
}

#[cfg(test)]
pub fn layers_index(layers: &[OwnedLayerSummary]) -> Html<String> {
    layers_index_with_forms(layers, "")
}

pub fn layers_index_with_forms(layers: &[OwnedLayerSummary], csrf: &str) -> Html<String> {
    layers_index_for_owner(layers, csrf, true)
}

pub fn layers_index_for_owner(
    layers: &[OwnedLayerSummary],
    csrf: &str,
    can_create: bool,
) -> Html<String> {
    let mut list = String::new();
    if layers.is_empty() {
        list.push_str(
            "<div class=\"author-layers-page__empty\"><h3>Пакетов пока нет</h3>
             <p>Создайте первый пакет, чтобы объединять книги и продавать доступ к ним.</p></div>",
        );
    } else {
        list.push_str("<ul class=\"author-layers-page__list\">");
        for layer in layers {
            let (status_label, status_class) = match layer.status.as_str() {
                "active" => ("Опубликован", "is-active"),
                "hidden" => ("Скрыт", "is-hidden"),
                "blocked" => ("Заблокирован", "is-blocked"),
                _ => ("Неизвестный статус", "is-unknown"),
            };
            write!(
                &mut list,
                r#"<li class="author-layer-card author-layer-card--entry">
                   <a class="author-layer-card__open" href="/me/layers/{id}/books">
                     <span class="author-layer-card__monogram" aria-hidden="true">К</span>
                     <span class="author-layer-card__identity">
                       <span class="author-layer-card__eyebrow">Пакет №{id}</span>
                       <strong>{title}</strong>
                       <span class="author-layer-card__summary">{description}</span>
                     </span>
                     <span class="author-layer-card__status {status_class}">{status_label}</span>
                     <span class="author-layer-card__arrow" aria-hidden="true">›</span>
                   </a>
                  </li>"#,
                id=layer.layer_id,
                title=escape_html(&layer.title),
                description=escape_html(&layer.description),
            )
            .expect("write package card");
        }
        list.push_str("</ul>");
        if layers.len() == 100 {
            list.push_str("<p class=\"author-layers-page__limit\">Показаны последние 100 пакетов.</p>");
        }
    }
    let content = include_str!("../templates/author_layers.html")
        .replace(
            "<!--ACCOUNT_NAVIGATION-->",
            &crate::pages::account_navigation("/me/layers", csrf),
        )
        .replace(
            "<!--CREATE_PACKAGE_ACTION-->",
            if can_create {
                r#"<a class="ui-button ui-button--primary" href="/me/layers/new">Создать пакет</a>"#
            } else {
                "Для создания пакета сначала нужна хотя бы одна собственная книга."
            },
        )
        .replace("<!--LAYER_COUNT-->", &layers.len().to_string())
        .replace("<!--LAYER_LIST-->", &list);
    layout("Мои пакеты", "Ваши книжные пакеты и их состав.", &content)
}

pub fn new_book(csrf: &str, command_key: &str) -> Html<String> {
    let content = include_str!("../templates/author_new_book.html")
        .replace(
            "<!--ACCOUNT_NAVIGATION-->",
            &crate::pages::account_navigation("/me/books/new", csrf),
        )
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout("Создать книгу", "Новая книга в вашем кабинете.", &content)
}

/// Generic duplicate response. Do not reveal whether another user's draft exists.
pub fn create_conflict() -> Html<String> {
    let content = r#"<section class="section auth-section">
      <div class="container"><div class="auth-card author-card" role="alert">
        <p class="eyebrow">Регистрация книги</p>
        <h1>Книга не создана</h1>
        <p>Такое название или ссылка на оригинал уже может использоваться другой книгой.
        Проверьте каталог. Если это ваша книга, откройте её в личном кабинете.
        Также проверьте, не меняли ли вы данные при повторной отправке формы.</p>
        <p><a class="button" href="/catalog">Проверить каталог</a></p>
        <p><a href="/me/books">Мои книги</a> ·
        <a href="/me/books/new">Попробовать с другим названием</a></p>
      </div></div>
    </section>"#;
    layout(
        "Книга не создана",
        "Обнаружено возможное совпадение книги.",
        content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_book_selects_creation_and_reuses_escaped_csrf_for_both_forms() {
        let html = new_book("<csrf>", "<command>").0;
        assert!(html.contains("href=\"/me/books/new\" aria-current=\"page\""));
        assert_eq!(html.matches("aria-current=\"page\"").count(), 1);
        assert!(!html.contains("aria-current=\"location\""));
        assert_eq!(
            html.matches("name=\"csrf_token\" value=\"&lt;csrf&gt;\"")
                .count(),
            2
        );
        assert!(html.contains("name=\"command_key\" value=\"&lt;command&gt;\""));
        assert!(!html.contains("<!--ACCOUNT_NAVIGATION-->"));
    }

    #[test]
    fn author_index_preserves_hostile_titles_navigation_and_the_list_limit() {
        let empty = index(&[], "<csrf>").0;
        assert!(empty.contains("У вас пока нет книг"));
        assert!(empty.contains("href=\"/me/books\" aria-current=\"page\""));
        assert!(empty.contains("name=\"csrf_token\" value=\"&lt;csrf&gt;\""));
        assert!(!empty.contains("author-workspace__book\""));
        let books: Vec<_> = (1..=100)
            .map(|id| OwnedBook {
                id,
                title: format!(
                    "{} <script>alert(1)</script>",
                    "Очень длинное название ".repeat(6)
                ),
            })
            .collect();
        let html = index(&books, "csrf").0;
        assert!(html.contains("Показаны последние 100 книг."));
        assert!(html.contains("/me/books/100/settings/delete"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn d_05_owned_layer_navigation_is_escaped_and_feature_scoped() {
        let layers = vec![OwnedLayerSummary {
            layer_id: 91,
            title: "Layer <script>alert(1)</script>".to_owned(),
            description: String::new(),
            status: "active".to_owned(),
            metadata_revision: 1,
        }];
        let visible = index_with_layers(&[], &layers, "csrf").0;
        assert!(visible.contains("/me/layers/91/books"));
        assert!(visible.contains("/me/layers/new"));
        assert!(visible.contains("href=\"/me/layers\""));
        assert!(visible.contains("Layer &lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!visible.contains("<script>alert(1)</script>"));
        assert!(!visible.contains("user_id"));
        let disabled = index(&[], "csrf").0;
        assert!(!disabled.contains("my-layers-heading"));
        assert!(!disabled.contains("/me/layers/91/books"));
    }

    #[test]
    fn d_05_packages_index_is_only_navigation_not_inline_edit_forms() {
        let layers = [
            OwnedLayerSummary {
                layer_id: 91,
                title: "<script>Не доверять</script>".into(),
                description: "<b>Содержание</b>".into(),
                status: "active".into(),
                metadata_revision: 4,
            },
            OwnedLayerSummary {
                layer_id: 92,
                title: "Скрытый".into(),
                description: "Не опубликован".into(),
                status: "hidden".into(),
                metadata_revision: 5,
            },
            OwnedLayerSummary {
                layer_id: 93,
                title: "Заблокированный".into(),
                description: String::new(),
                status: "blocked".into(),
                metadata_revision: 6,
            },
        ];
        let html = layers_index_with_forms(&layers, "<csrf>").0;
        assert!(html.contains("class=\"account-sidebar\""));
        assert!(html.contains("В списке: 3"));
        assert!(html.contains("href=\"/me/layers/91/books\""));
        assert!(html.contains("href=\"/me/layers/92/books\""));
        assert!(html.contains("href=\"/me/layers/93/books\""));
        assert!(!html.contains("action=\"/me/layers/91/metadata\""));
        assert!(!html.contains("name=\"expected_revision\""));
        assert!(html.contains("&lt;script&gt;Не доверять&lt;/script&gt;"));
        assert!(html.contains("&lt;b&gt;Содержание&lt;/b&gt;"));
        assert!(!html.contains("<script>Не доверять</script>"));
        assert!(!html.contains("user_id"));
        let empty = layers_index_with_forms(&[], "csrf").0;
        assert!(empty.contains("Пакетов пока нет"));
    }

    #[test]
    fn d_05_owner_layer_cards_are_escaped_and_only_link_to_details() {
        let layers = vec![OwnedLayerSummary {
            layer_id: 91,
            title: "Слой <script>alert(1)</script>".to_owned(),
            description: "<b>Описание</b>".to_owned(),
            status: "hidden".to_owned(),
            metadata_revision: 2,
        }];
        let html = layers_index(&layers).0;
        assert!(html.contains("Мои пакеты"));
        assert!(html.contains("href=\"/me/layers/new\""));
        assert!(html.contains("href=\"/me/layers/91/books\""));
        assert!(html.contains("Слой &lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;b&gt;Описание&lt;/b&gt;"));
        assert!(!html.contains("action=\"/me/layers/91/metadata\""));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("user_id"));
        assert!(!html.contains("wallet"));
    }
}
