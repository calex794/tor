use crate::{auth_cookies::CookiePolicy, pages};
use axum::{http::HeaderMap, response::Html};
use std::{sync::Arc, time::Duration};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_ads::{
    domain::editorial::{EditorialCard, Viewer},
    repositories::editorial::PgEditorial,
    services::editorial::Editorial,
};
use tor_contracts::exposure::{
    CardObservation, EditorialPlacement, ObservationIssuer, PromotionSlot,
};

pub(crate) const MARKER: &str = "<!--EDITORIAL_PROMOTION-->";
pub(crate) type Issuer = Option<axum::Extension<Arc<dyn ObservationIssuer>>>;
pub(crate) fn issuer(value: &Issuer) -> Option<&dyn ObservationIssuer> {
    value.as_ref().map(|extension| extension.0.as_ref())
}
pub(crate) fn configured(
    catalog: Option<&tor_books::repositories::PgCatalog>,
    control: Option<&crate::book_follow_page::FollowBookControl>,
) -> Option<DisplayRuntime> {
    let catalog = catalog?;
    let authority = control.map(|control| {
        let (accounts, cookies) = control.browser_context();
        (accounts.clone(), cookies.clone())
    });
    Some(DisplayRuntime::new(
        PgEditorial::new(catalog.pool()),
        authority,
    ))
}

pub(crate) async fn book(
    state: &crate::AppState,
    headers: &HeaderMap,
    id: i64,
    html: Html<String>,
    observation: Issuer,
) -> Html<String> {
    if let Some(editorial) = &state.editorial {
        editorial
            .attach(
                headers,
                PromotionSlot::BookDetailRelated,
                Some(id),
                html,
                issuer(&observation),
            )
            .await
    } else {
        html
    }
}

#[derive(Clone)]
pub(crate) struct DisplayRuntime {
    store: PgEditorial,
    authority: Option<(PgAccounts, CookiePolicy)>,
    enabled: bool,
}
impl DisplayRuntime {
    pub(crate) fn new(store: PgEditorial, authority: Option<(PgAccounts, CookiePolicy)>) -> Self {
        Self {
            store,
            authority,
            enabled: super::enabled(),
        }
    }
    pub(crate) async fn attach(
        &self,
        headers: &HeaderMap,
        slot: PromotionSlot,
        host: Option<i64>,
        html: Html<String>,
        observation: Option<&dyn ObservationIssuer>,
    ) -> Html<String> {
        if !self.enabled
            || (slot == PromotionSlot::ChapterEndFree
                && std::env::var("TOR_RECOMMENDATIONS_ENABLED").as_deref() == Ok("1"))
        {
            // The 50/40/10 shelf owns the chapter-end inventory when REC is
            // enabled. Do not render a second editorial block above it.
            return Html(html.0.replace(MARKER, ""));
        }
        let operation = async {
            let viewer = if let Some((accounts, cookies)) = &self.authority {
                if let Some(token) = cookies.session_token(headers) {
                    let session_digest = session_token_digest(&token);
                    let actor = accounts.session_actor(&session_digest).await.ok()??;
                    Some(Viewer {
                        user_id: actor.actor.user_id.get(),
                        session_digest,
                    })
                } else {
                    None
                }
            } else {
                None
            };
            Editorial::new(&self.store)
                .cards(viewer.as_ref(), slot, host)
                .await
                .ok()
        };
        // Optional promotion never changes availability or latency budget of the reader.
        let cards = tokio::time::timeout(Duration::from_millis(400), operation)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        Html(html.0.replace(MARKER, &render_cards(&cards, observation)))
    }
}

/// Only a trusted server renderer can ask the shared issuer for an opaque proof.
/// Preview deliberately supplies None; arbitrary HTML data attributes are not read.
pub(super) fn render_cards(
    cards: &[EditorialCard],
    issuer: Option<&dyn ObservationIssuer>,
) -> String {
    let mut items = String::new();
    for card in cards.iter().take(4) {
        let proof = issuer
            .and_then(|issuer| {
                issuer.issue(CardObservation {
                    book_id: card.book_id,
                    surface: card.slot.surface(),
                    editorial: Some(EditorialPlacement {
                        campaign_id: card.campaign_id,
                        revision: card.revision,
                        slot: card.slot,
                    }),
                })
            })
            .map(|proof| format!(" data-flow-proof=\"{}\"", pages::escape_html(&proof)))
            .unwrap_or_default();
        items.push_str(&format!("<article class=\"book-section-card editorial-card\" data-editorial-campaign=\"{}\"{proof}><p class=\"book-section-card__kicker\">Выбор редакции</p><h3 class=\"book-section-card__title\"><a data-flow-link href=\"/books/{}\">{}</a></h3><p>Открыть книгу</p></article>",card.campaign_id,card.book_id,pages::escape_html(&card.title)));
    }
    if items.is_empty() {
        return items;
    }
    format!("<section class=\"section editorial-promotion\" aria-label=\"Редакционное продвижение\"><div class=\"container\"><h2>Выбор редакции</h2><div class=\"book-related-grid\">{items}</div></div></section>")
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Issuer;
    impl ObservationIssuer for Issuer {
        fn issue(&self, card: CardObservation) -> Option<String> {
            assert_eq!(card.editorial.unwrap().slot, PromotionSlot::HomeFeatured);
            Some("proof&safe".into())
        }
    }
    #[test]
    fn preview_does_not_mint_measurements_and_markup_is_escaped() {
        let cards = vec![EditorialCard {
            campaign_id: 4,
            revision: 2,
            book_id: 9,
            title: "<script>&".into(),
            slot: PromotionSlot::HomeFeatured,
        }];
        let preview = render_cards(&cards, None);
        assert!(!preview.contains("data-flow-proof"));
        assert!(!preview.contains("<script>"));
        assert!(preview.contains("&lt;script&gt;&amp;"));
        assert!(render_cards(&cards, Some(&Issuer)).contains("data-flow-proof=\"proof&amp;safe\""));
    }
}
