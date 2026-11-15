use crate::{
    auth_cookies::{cookie_value, CookiePolicy},
    pages::escape_html as esc,
};
use axum::http::HeaderMap;
use std::sync::Arc;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_ads::{
    domain::paid::AdFlags,
    repositories::paid::PgPaidAds,
    services::console::{AdConsoleStore, AdDeliveryStore},
};
use tor_contracts::{ads::AdActor, exposure::PromotionSlot};

#[derive(Clone)]
pub(crate) struct PaidDisplay {
    store: Arc<dyn AdDeliveryStore>,
    console: Arc<dyn AdConsoleStore>,
    flags: AdFlags,
}
impl PaidDisplay {
    pub(crate) fn new(accounts: &PgAccounts) -> Self {
        let store = Arc::new(PgPaidAds::new(accounts.pool().clone()));
        Self {
            console: store.clone(),
            store,
            flags: super::flags(),
        }
    }
    pub(crate) fn active(&self) -> bool {
        self.flags.daily_serving || self.flags.chapter_serving
    }
    pub(crate) async fn choose(
        &self,
        _headers: &HeaderMap,
        actor: Option<&AdActor>,
        slot: PromotionSlot,
        host: Option<i64>,
        chapter: Option<i64>,
        has_editorial: bool,
    ) -> Option<String> {
        if !self.flags.daily_serving && !self.flags.chapter_serving {
            return None;
        }
        let (paid_first, mut cards) = self
            .store
            .cards(actor, slot, host, self.flags, has_editorial)
            .await
            .ok()?;
        if cards.is_empty() || (has_editorial && !paid_first) {
            return None;
        }
        let shelf = slot == PromotionSlot::ChapterEndFree;
        let mut out = if shelf {
            String::from(
                r#"<section class="section reader-end-shelf paid-promotion" data-rec-panel aria-label="Книги после главы"><div class="container"><h2>Ещё книги</h2><div class="home-shelf__track" aria-label="Подборка книг">"#,
            )
        } else {
            String::from(
                r#"<section class="section paid-promotion" aria-label="Реклама книг"><div class="container"><h2>Реклама книг</h2><div class="book-related-grid">"#,
            )
        };
        for card in &mut cards {
            if self.flags.billable {
                if let (Some(actor), Some(assignment), Some(chapter)) =
                    (actor, card.assignment_id, chapter)
                {
                    let token = generate_session_token();
                    if self
                        .store
                        .issue_opportunity(actor, assignment, chapter, &token)
                        .await
                        .is_ok()
                    {
                        card.opportunity_token = Some(token);
                    }
                }
            }
            let proof = card
                .opportunity_token
                .as_ref()
                .map(|token| format!(" data-paid-ad-token=\"{}\"", esc(token)))
                .unwrap_or_default();
            if shelf {
                let markup =
                    crate::recommendation_http::shelf_card::markup(card.book_id, &card.title, None);
                out.push_str(&format!(r#"<article class="book-mini-card book-mini-card--shelf paid-ad-card" data-paid-campaign="{}" data-paid-book="{}" data-paid-product="{}" data-paid-generation="{}" data-paid-assignment="{}"{proof}>{markup}<span class="ad-label">Реклама</span></article>"#,card.campaign_id,card.book_id,esc(&card.product),card.generation_id.unwrap_or(0),card.assignment_id.unwrap_or(0)));
            } else {
                out.push_str(&format!(r#"<article class="book-section-card paid-ad-card" data-paid-campaign="{}" data-paid-book="{}" data-paid-product="{}"><p class="book-section-card__kicker">Реклама</p><h3 class="book-section-card__title"><a href="/books/{}">{}</a></h3></article>"#,card.campaign_id,card.book_id,esc(&card.product),card.book_id,esc(&card.title)));
            }
        }
        if shelf {
            out.push_str("<!--READER_END_ORGANIC_CARDS-->");
        }
        out.push_str("</div></div></section><script src=\"/assets/paid-ads.js\" defer></script>");
        out.insert_str(0, "<link rel=\"stylesheet\" href=\"/assets/paid-ads.css\">");
        Some(out)
    }
    pub(crate) async fn record_open(
        &self,
        headers: &HeaderMap,
        authority: &Option<(PgAccounts, CookiePolicy)>,
        book: i64,
    ) {
        let Some(token) = cookie_value(headers, "tor-ad-context") else {
            return;
        };
        let Some((accounts, cookies)) = authority else {
            return;
        };
        let Some(session) = cookies.session_token(headers) else {
            return;
        };
        let digest = session_token_digest(&session);
        let Ok(Some(actor)) = accounts.session_actor(&digest).await else {
            return;
        };
        let a = AdActor {
            user_id: actor.actor.user_id.get(),
            session_digest: digest,
        };
        let _ = self.console.open(&a, &token, book).await;
    }
}
