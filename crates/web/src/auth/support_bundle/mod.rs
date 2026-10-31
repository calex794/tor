//! E-11 support: shared route scope for separate support modules.
use super::{deny, session_user, AuthState};
use super::login_limiter;
use axum::Router;

#[path = "../support.rs"]
mod support;
#[path = "../support_admin.rs"]
mod support_admin;
#[path = "../support_identity.rs"]
mod support_identity;
#[path = "../support_pages.rs"]
mod support_pages;

pub(super) fn router() -> Router<AuthState> {
    support::router()
}
