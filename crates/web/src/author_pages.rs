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
        }];
        let visible = index_with_layers(&[], &layers).0;
        assert!(visible.contains("/me/layers/91/books"));
        assert!(visible.contains("Layer &lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!visible.contains("<script>alert(1)</script>"));
        assert!(!visible.contains("user_id"));
        let disabled = index(&[]).0;
        assert!(!disabled.contains("my-layers-heading"));
        assert!(!disabled.contains("/me/layers/91/books"));
    }
}
