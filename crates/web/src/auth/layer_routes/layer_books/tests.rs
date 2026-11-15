//! Package picker unit tests (kept separate from the HTTP handler).
use super::*;
use tor_commerce::domain::layers::LayerBookCandidate;

fn current_empty() -> LayerCurrentBookPage {
    LayerCurrentBookPage {
        current_version_id: 42,
        books: vec![],
        next_after: None,
    }
}

fn offer_empty() -> LayerOwnerOfferState {
    LayerOwnerOfferState {
        current_version_id: 42,
        current_offer_id: None,
        term_kind: None,
        price_minor: None,
    }
}

#[test]
fn d_05_picker_escapes_untrusted_book_titles_and_excludes_user_identity() {
    let page = LayerBookCandidatePage {
        books: vec![LayerBookCandidate {
            book_id: 91,
            title: "<script>alert('x')</script>".into(),
        }],
        next_after: Some(91),
    };
    let html = picker_html(9, &page, &current_empty(), &offer_empty(), "test_csrf").0;
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert!(html.contains("name=\"book_id\" value=\"91\""));
    assert!(html.contains("name=\"csrf_token\""));
    assert!(html.contains("books?after=91"));
    assert!(!html.contains("user_id"));
    assert!(!html.contains("share_percent"));
}

#[test]
fn d_05_empty_picker_has_no_add_book_form() {
    let html = picker_html(
        9,
        &LayerBookCandidatePage {
            books: vec![],
            next_after: None,
        },
        &current_empty(),
        &offer_empty(),
        "token",
    )
    .0;
    assert!(html.contains("Подходящих книг не найдено"));
    assert!(!html.contains("name=\"book_id\""));
}
#[test]
fn d_05_offer_form_is_cas_bound_and_never_contains_wallet_actor() {
    let mut offer = offer_empty();
    let html = offer_form_html(9, &offer, "<csrf>");
    assert!(html.contains("action=\"/me/layers/9/offer\""));
    assert!(html.contains("name=\"expected_version_id\" value=\"42\""));
    assert!(html.contains("name=\"expected_offer_id\" value=\"0\""));
    assert!(html.contains("&lt;csrf&gt;"));
    assert!(!html.contains("wallet_id"));
    assert!(!html.contains("user_id"));
    offer.current_offer_id = Some(31);
    offer.term_kind = Some(LayerTermKind::Lifetime);
    offer.price_minor = Some(14999);
    let html = offer_form_html(9, &offer, "csrf");
    assert!(html.contains("value=\"149,99\""));
    assert!(html.contains("name=\"expected_offer_id\" value=\"31\""));
    assert!(!html.contains("value=\"prepaid_30d\""));
}

#[test]
fn package_search_is_escaped_and_keeps_utf8_query_in_navigation() {
    let page = LayerBookCandidatePage {
        books: vec![LayerBookCandidate {
            book_id: 91,
            title: "Книга".into(),
        }],
        next_after: Some(91),
    };
    let html = picker_html_with_query(
        9,
        &page,
        &current_empty(),
        &offer_empty(),
        "csrf",
        "Книга & <script>",
        true,
    )
    .0;
    assert!(html.contains("value=\"Книга &amp; &lt;script&gt;\""));
    assert!(html.contains("q=%D0%9A%D0%BD%D0%B8%D0%B3%D0%B0"));
    assert!(!html.contains("value=\"Книга & <script>\""));
}

#[test]
fn hidden_package_is_read_only_even_with_current_books_and_offer() {
    let current = LayerCurrentBookPage {
        current_version_id: 42,
        books: vec![LayerBookCandidate {
            book_id: 91,
            title: "Закрытая книга".into(),
        }],
        next_after: None,
    };
    let html = picker_html_with_query(
        9,
        &LayerBookCandidatePage {
            books: vec![],
            next_after: None,
        },
        &current,
        &offer_empty(),
        "csrf",
        "",
        false,
    )
    .0;
    assert!(html.contains("Закрытая книга"));
    assert!(html.contains("Пакет скрыт от новых покупок"));
    assert!(!html.contains("action=\"/me/layers/9/books\""));
    assert!(!html.contains("action=\"/me/layers/9/offer\""));
    assert!(!html.contains("name=\"price_rub\""));
}

#[test]
fn d_05_current_layer_book_renders_version_bound_remove_form() {
    let page = LayerCurrentBookPage {
        current_version_id: 42,
        books: vec![tor_commerce::domain::layers::LayerBookCandidate {
            book_id: 91,
            title: "<img onerror=alert(1)>".into(),
        }],
        next_after: Some(91),
    };
    let html = picker_html(
        9,
        &LayerBookCandidatePage {
            books: vec![],
            next_after: None,
        },
        &page,
        &offer_empty(),
        "test_csrf",
    )
    .0;
    assert!(html.contains("name=\"operation\" value=\"remove\""));
    assert!(html.contains("name=\"expected_version_id\" value=\"42\""));
    assert!(html.contains("name=\"book_id\" value=\"91\""));
    assert!(html.contains("&lt;img onerror=alert(1)&gt;"));
    assert!(!html.contains("<img onerror="));
    assert!(html.contains("current_after=91"));
}
