//! Shared account router and handler forms.
use super::*;

#[derive(Clone)]
pub(super) struct AuthState {
    pub(super) accounts: PgAccounts,
    pub(super) checkout: tor_commerce::repositories::checkout::PgCheckout,
    pub(super) paid_checkout_enabled: bool,
    pub(super) paid_reader_available: bool,
    pub(super) authoring: PgAuthoring,
    pub(super) progress: PgReadingProgress,
    pub(super) sync: PgReadingSync,
    pub(super) bookmarks: PgLibrary,
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
    pub(super) limit: Option<u16>,
}

#[derive(Deserialize)]
pub(super) struct NewBookForm {
    pub(super) title: String,
    pub(super) target_language: String,
    pub(super) author_name: Option<String>,
    pub(super) genres: Option<String>,
    pub(super) tags: Option<String>,
    pub(super) fandoms: Option<String>,
    pub(super) command_key: String,
    pub(super) csrf_token: Option<String>,
}

