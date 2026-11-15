//! Package picker unit tests (kept separate from the HTTP handler).
use super::draft::stage_choices;
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

#[test]
fn composition_grid_delta_is_limited_to_visible_books() {
    let form: AddBookForm = serde_json::from_value(serde_json::json!({
        "operation": "preview",
        "visible_csv": "1,2,3",
        "included_csv": "1,2",
        "chosen_1": "1",
        "chosen_3": "1"
    }))
    .unwrap();
    assert_eq!(staged_changes(&form), Some((vec![3], vec![2])));
    let forged: AddBookForm = serde_json::from_value(serde_json::json!({
        "visible_csv": "1,2,3",
        "included_csv": "1,2",
        "chosen_999": "1"
    }))
    .unwrap();
    assert!(staged_changes(&forged).is_none());
}

#[test]
fn grid_has_book_covers_and_does_not_trust_titles_or_share_ids() {
    use tor_commerce::{
        domain::layers::OwnedLayerSummary,
        repositories::layer_composition::{LayerBookTile, LayerBookTilePage},
    };
    let layer = OwnedLayerSummary {
        layer_id: 9,
        title: "<script>Пакет</script>".into(),
        description: "<b>Описание</b>".into(),
        status: "active".into(),
        metadata_revision: 2,
    };
    let tiles = LayerBookTilePage {
        books: vec![
            LayerBookTile {
                id: 91,
                title: "<img onerror=alert(1)>".into(),
                has_cover: true,
                included: true,
            },
            LayerBookTile {
                id: 92,
                title: "Новая книга".into(),
                has_cover: false,
                included: false,
            },
        ],
        next_after: None,
        hero_cover_id: Some(91),
    };
    let draft = tor_commerce::repositories::layer_composition_drafts::LayerCompositionDraft {
        version_id: 42,
        revision: 2,
        additions: vec![92],
        removals: vec![91],
    };
    let (hero, metadata, grid) = grid::detail(
        &layer,
        &tiles,
        &offer_empty(),
        grid::GridOptions {
            csrf: "csrf",
            query: "",
            filter: "all",
            after: None,
            history: &[],
            draft: &draft,
            editable: true,
            cover: grid::GridCoverState::default(),
        },
    );
    assert!(hero.contains("&lt;script&gt;Пакет&lt;/script&gt;"));
    assert!(hero.contains("/covers/91/card.webp"));
    assert!(!hero.contains("href=\"/layers/9\""));
    assert!(!hero.contains("<script>Пакет</script>"));
    assert!(metadata.contains("expected_revision"));
    assert!(grid.contains("/covers/91/card.webp"));
    assert!(grid.contains("chosen_91"));
    assert!(grid.contains("chosen_92"));
    assert!(grid.contains("&lt;img onerror=alert(1)&gt;"));
    assert!(!grid.contains("<img onerror=alert(1)>"));
    assert!(!grid.contains("user_id"));
}

#[test]
fn package_draft_stage_rejects_foreign_and_duplicate_choices() {
    let form: AddBookForm = serde_json::from_value(serde_json::json!({
        "operation": "stage",
        "visible_csv": "7,8,9",
        "chosen_8": "1"
    }))
    .unwrap();
    assert_eq!(stage_choices(&form), Some((vec![7, 8, 9], vec![8])));
    let forged: AddBookForm = serde_json::from_value(serde_json::json!({
        "visible_csv": "7,8",
        "chosen_999": "1"
    }))
    .unwrap();
    assert!(stage_choices(&forged).is_none());
    let duplicated: AddBookForm = serde_json::from_value(serde_json::json!({
        "visible_csv": "7,7",
        "chosen_7": "1"
    }))
    .unwrap();
    assert!(stage_choices(&duplicated).is_none());
    assert_eq!(history_ids(Some("0,91,200")), Some(vec![0, 91, 200]));
    assert_eq!(history_ids(Some("-1")), None);
    assert_eq!(history_ids(Some("1,invalid")), None);
    assert!(valid_filter("included"));
    assert!(!valid_filter("<script>"));
}

#[test]
fn package_grid_keeps_saved_draft_across_filter_navigation() {
    use tor_commerce::{
        domain::layers::OwnedLayerSummary,
        repositories::{
            layer_composition::{LayerBookTile, LayerBookTilePage},
            layer_composition_drafts::LayerCompositionDraft,
        },
    };
    let layer = OwnedLayerSummary {
        layer_id: 7,
        title: "Draft".into(),
        description: String::new(),
        status: "active".into(),
        metadata_revision: 1,
    };
    let draft = LayerCompositionDraft {
        version_id: 42,
        revision: 4,
        additions: vec![121],
        removals: vec![120],
    };
    let tiles = LayerBookTilePage {
        books: vec![
            LayerBookTile {
                id: 120,
                title: "Old".into(),
                has_cover: false,
                included: true,
            },
            LayerBookTile {
                id: 121,
                title: "New".into(),
                has_cover: false,
                included: false,
            },
        ],
        next_after: Some(121),
        hero_cover_id: None,
    };
    let (_, _, grid) = grid::detail(
        &layer,
        &tiles,
        &offer_empty(),
        grid::GridOptions {
            csrf: "csrf",
            query: "Chapter",
            filter: "included",
            after: Some(99),
            history: &[0, 50],
            draft: &draft,
            editable: true,
            cover: grid::GridCoverState::default(),
        },
    );
    assert!(grid.contains("name=\"draft_revision\" value=\"4\""));
    assert!(grid.contains("name=\"after\" value=\"99\""));
    assert!(grid.contains("name=\"history_csv\" value=\"0,50\""));
    assert!(grid.contains("name=\"filter\" value=\"included\""));
    assert!(grid.contains("name=\"nav\" value=\"review\""));
    assert!(grid.contains("name=\"nav\" value=\"previous\""));
    assert!(grid.contains("name=\"nav\" value=\"next\""));
    assert!(grid.contains("Будет убрана"));
    assert!(grid.contains("Будет добавлена"));
    assert!(grid.contains("name=\"chosen_121\" value=\"1\" checked"));
}

#[test]
fn package_cover_is_uploaded_only_with_owner_storage_and_previews_stay_private() {
    use tor_commerce::{
        domain::layers::OwnedLayerSummary,
        repositories::{
            layer_composition::LayerBookTilePage, layer_composition_drafts::LayerCompositionDraft,
        },
    };
    let layer = OwnedLayerSummary {
        layer_id: 5,
        title: "Pack".into(),
        description: String::new(),
        status: "active".into(),
        metadata_revision: 1,
    };
    let tiles = LayerBookTilePage {
        books: vec![],
        next_after: None,
        hero_cover_id: None,
    };
    let draft = LayerCompositionDraft {
        version_id: 9,
        revision: 0,
        additions: vec![],
        removals: vec![],
    };
    let render = |cover| {
        grid::detail(
            &layer,
            &tiles,
            &offer_empty(),
            grid::GridOptions {
                csrf: "csrf",
                query: "",
                filter: "all",
                after: None,
                history: &[],
                draft: &draft,
                editable: true,
                cover,
            },
        )
        .0
    };
    let without_store = render(grid::GridCoverState::default());
    assert!(without_store.contains("Загрузка отдельной обложки недоступна"));
    assert!(!without_store.contains("enctype=\"multipart/form-data\""));
    let ready = render(grid::GridCoverState {
        upload_enabled: true,
        ..grid::GridCoverState::default()
    });
    assert!(ready.contains("action=\"/me/layers/5/cover\""));
    assert!(ready.contains("name=\"csrf_token\" value=\"csrf\""));
    let pending = render(grid::GridCoverState {
        pending: true,
        upload_enabled: true,
        ..grid::GridCoverState::default()
    });
    assert!(pending.contains("/me/layers/5/cover/preview"));
    assert!(!pending.contains("enctype=\"multipart/form-data\""));
    let published = render(grid::GridCoverState {
        approved: true,
        upload_enabled: true,
        ..grid::GridCoverState::default()
    });
    assert!(published.contains("/me/layers/5/cover/approved"));
}
