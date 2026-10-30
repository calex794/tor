//! Private author book pages. No fake books and no client-supplied user IDs.
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::OwnedBook;
use tor_commerce::domain::layers::OwnedLayerSummary;

use crate::pages::{escape_html, layout};

pub fn index(books: &[OwnedBook]) -> Html<String> {
    render_index(books, None)
}

/// The Layer section is visible only when the author management feature
/// is explicitly enabled; no purchase or subscription is implied.
pub fn index_with_layers(books: &[OwnedBook], layers: &[OwnedLayerSummary]) -> Html<String> {
    render_index(books, Some(layers))
}

fn render_index(books: &[OwnedBook], layers: Option<&[OwnedLayerSummary]>) -> Html<String> {
    let mut list = String::new();
    if books.is_empty() {
        list.push_str("<p class=\"empty-state\">У вас пока нет книг. Создайте первую.</p>");
    } else {
        list.push_str("<ul class=\"author-book-list\">");
        for book in books {
            write!(
                &mut list,
                "<li><a class=\"author-book-link\" href=\"/me/books/{}\">{}</a></li>",
                book.id,
                escape_html(&book.title)
            )
            .expect("write String");
        }
        list.push_str("</ul>");
        if books.len() == 100 {
            list.push_str("<p>Показаны последние 100 книг.</p>");
        }
    }
    let layer_section = layers.map_or_else(String::new, |owned| {
        let mut section = String::from(
            "<section aria-labelledby=\"my-layers-heading\">\
             <h2 id=\"my-layers-heading\">Мои Layer</h2>",
        );
        section.push_str(
            "<p><a class=\"button\" href=\"/me/layers/new\">Создать слой</a> <a class=\"text-link\" href=\"/me/layers\">Все мои слои</a></p>",
        );
        if owned.is_empty() {
            section.push_str("<p class=\"empty-state\">Слоёв пока нет.</p>");
        } else {
            section.push_str("<ul class=\"author-book-list\">");
            for layer in owned {
                write!(
                    section,
                    "<li><a class=\"author-book-link\" href=\"/me/layers/{}/books\">{} — выбрать книги</a></li>",
                    layer.layer_id,
                    escape_html(&layer.title)
                )
                .expect("write String");
            }
            section.push_str("</ul>");
            if owned.len() == 100 {
                section.push_str("<p>Показаны последние 100 Layer.</p>");
            }
        }
        section.push_str("</section>");
        section
    });
    let content = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-card">
        <p class="eyebrow">Кабинет автора</p><h1>Мои книги</h1>
        <p><a class="button" href="/me/books/new">Создать книгу</a></p>
        {list}
        {layer_section}
        <p><a href="/me">Вернуться в кабинет</a></p>
        </div></div></section>"#
    );
    layout("Мои книги", "Ваши книги из PostgreSQL.", &content)
}

#[cfg(test)]
pub fn layers_index(layers: &[OwnedLayerSummary]) -> Html<String> {
    layers_index_with_forms(layers, "")
}

pub fn layers_index_with_forms(layers: &[OwnedLayerSummary], csrf: &str) -> Html<String> {
    let mut list = String::new();
    if layers.is_empty() {
        list.push_str("<p class=\"empty-state\">Слоёв пока нет. Создайте первый.</p>");
    } else {
        list.push_str("<ul class=\"author-book-list\">");
        for layer in layers {
            let status_label = match layer.status.as_str() {
                "active" => "Опубликован",
                "hidden" => "Скрыт",
                "blocked" => "Заблокирован",
                _ => "Неизвестный статус",
            };
            write!(
                list,
                "<li><strong>{}</strong> — {}",
                escape_html(&layer.title),
                status_label,
            )
            .expect("write String");
            if layer.status == "active" || layer.status == "hidden" {
                let option = if layer.status == "active" {
                    "<option value=\"active\" selected>Опубликован</option><option value=\"hidden\">Скрыт</option>"
                } else {
                    "<option value=\"active\">Опубликован</option><option value=\"hidden\" selected>Скрыт</option>"
                };
                write!(
                    list,
                    "<form method=\"post\" action=\"/me/layers/{}/metadata\">\
                     <label>Название<input name=\"title\" maxlength=\"120\" required value=\"{}\"></label>\
                     <label>Описание<textarea name=\"description\" maxlength=\"2000\">{}</textarea></label>\
                     <label>Видимость<select name=\"status\">{}</select></label>\
                     <input type=\"hidden\" name=\"expected_revision\" value=\"{}\">\
                     <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">\
                     <button class=\"button\" type=\"submit\">Сохранить</button></form>\
                     <a href=\"/me/layers/{}/books\">Книги и тариф</a>",
                    layer.layer_id,
                    escape_html(&layer.title),
                    escape_html(&layer.description),
                    option,
                    layer.metadata_revision,
                    escape_html(csrf),
                    layer.layer_id
                )
                .expect("write String");
            } else {
                list.push_str("<p>Заблокированный слой нельзя изменить самостоятельно.</p>");
            }
            list.push_str("</li>");
        }
        list.push_str("</ul>");
        if layers.len() == 100 {
            list.push_str("<p>Показаны последние 100 слоёв.</p>");
        }
    }
    let content = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-card">
        <p class="eyebrow">Кабинет автора</p><h1>Мои слои</h1>
        <p>Управляйте составом книг и условиями доступа.</p>
        <p><a class="button" href="/me/layers/new">Создать слой</a></p>
        {list}
        <p><a href="/me/books">Мои книги</a> · <a href="/me">Вернуться в кабинет</a></p>
        </div></div></section>"#
    );
    layout("Мои слои", "Ваши слои и их состав.", &content)
}

pub fn new_book(csrf: &str, command_key: &str) -> Html<String> {
    let content = include_str!("../templates/author_new_book.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout("Создать книгу", "Новая книга в вашем кабинете.", &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_05_owned_layer_navigation_is_escaped_and_feature_scoped() {
        let layers = vec![OwnedLayerSummary {
            layer_id: 91,
            title: "Layer <script>alert(1)</script>".to_owned(),
            description: String::new(),
            status: "active".to_owned(),
            metadata_revision: 1,
        }];
        let visible = index_with_layers(&[], &layers).0;
        assert!(visible.contains("/me/layers/91/books"));
        assert!(visible.contains("/me/layers/new"));
        assert!(visible.contains("href=\"/me/layers\""));
        assert!(visible.contains("Layer &lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!visible.contains("<script>alert(1)</script>"));
        assert!(!visible.contains("user_id"));
        let disabled = index(&[]).0;
        assert!(!disabled.contains("my-layers-heading"));
        assert!(!disabled.contains("/me/layers/91/books"));
    }

    #[test]
    fn d_05_owner_layer_index_escapes_titles_and_hides_buyer_data() {
        let layers = vec![OwnedLayerSummary {
            layer_id: 91,
            title: "Слой <script>alert(1)</script>".to_owned(),
            description: "<b>Описание</b>".to_owned(),
            status: "hidden".to_owned(),
            metadata_revision: 2,
        }];
        let html = layers_index(&layers).0;
        assert!(html.contains("Мои слои"));
        assert!(html.contains("href=\"/me/layers/new\""));
        assert!(html.contains("href=\"/me/layers/91/books\""));
        assert!(html.contains("Слой &lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;b&gt;Описание&lt;/b&gt;"));
        assert!(html.contains("name=\"expected_revision\" value=\"2\""));
        let edited = layers_index_with_forms(&layers, "<csrf>").0;
        assert!(edited.contains("name=\"csrf_token\" value=\"&lt;csrf&gt;\""));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("user_id"));
        assert!(!html.contains("wallet"));
        let empty = layers_index(&[]).0;
        assert!(empty.contains("Слоёв пока нет"));
        let blocked = layers_index_with_forms(
            &[OwnedLayerSummary {
                layer_id: 92,
                title: "Блокировка".to_owned(),
                description: String::new(),
                status: "blocked".to_owned(),
                metadata_revision: 1,
            }],
            "csrf",
        )
        .0;
        assert!(blocked.contains("Заблокированный слой"));
        assert!(!blocked.contains("action=\"/me/layers/92/metadata\""));
    }
}
