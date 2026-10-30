//! Shared account router and handler forms.
use super::*;

/// Resolve actor from the session database, never client-supplied user IDs.
pub(super) async fn session_user(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<Option<i64>, StatusCode> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

#[derive(Clone)]
pub(super) struct AuthState {
    pub(super) referrals: tor_referrals::repositories::PgReferrals,
    pub(super) accounts: PgAccounts,
    pub(super) reset_delivery: Option<super::reset_delivery::ResetDelivery>,
    pub(super) notification_preferences: tor_reading::repositories::PgNotificationPreferences,
    pub(super) checkout: tor_commerce::repositories::checkout::PgCheckout,
    pub(super) paid_checkout_enabled: bool,
    pub(super) paid_reader_available: bool,
    pub(super) authoring: PgAuthoring,
    pub(super) progress: PgReadingProgress,
    pub(super) sync: PgReadingSync,
    pub(super) bookmarks: PgLibrary,
    pub(super) demand: PgContinuationDemand,
    pub(super) book_follows: tor_reading::repositories::PgBookFollows,
    pub(super) follow_updates: PgFollowUpdates,
    pub(super) reader: PgReader,
    pub(super) rankings: tor_recommendations::repositories::PgRanking,
    pub(super) cookies: CookiePolicy,
    pub(super) password_jobs: PasswordJobs,
    pub(super) ril_signer: Option<RilSubjectSigner>,
    pub(super) cover_jobs: Option<PgCoverJobs>,
    pub(super) cover_upload_enabled: bool,
}

#[derive(Deserialize)]
pub(super) struct LogoutForm {
    pub(super) csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct SavePlaceForm {
    pub(super) csrf_token: Option<String>,
    pub(super) version_id: u64,
    pub(super) block_index: i32,
    pub(super) char_offset: i32,
}

#[derive(Default, Deserialize)]
pub(super) struct LibraryQuery {
    pub(super) after: Option<i64>,
    pub(super) updates_after: Option<String>,
    pub(super) limit: Option<u16>,
}

#[derive(Deserialize)]
pub(crate) struct NewBookForm {
    pub(crate) title: String,
    pub(crate) target_language: String,
    pub(crate) original_title: Option<String>,
    pub(crate) alternative_titles: Option<String>,
    pub(crate) author_name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) genres: Option<String>,
    pub(crate) tags: Option<String>,
    pub(crate) fandoms: Option<String>,
    pub(crate) command_key: String,
    pub(crate) csrf_token: Option<String>,
}
