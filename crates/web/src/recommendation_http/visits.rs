//! Retired individual navigation collector. Existing reader adapters retain
//! their signatures without session lookups, a queue or new attribution facts.
use crate::auth_cookies::CookiePolicy;
use axum::{response::Response, Router};
use tor_accounts::repositories::PgAccounts;

#[derive(Clone)]
pub(crate) struct Context;

impl Context {
    pub(crate) fn new(_accounts: PgAccounts, _cookies: CookiePolicy) -> Self {
        Self
    }
}

pub(crate) fn instrument(app: Router, _context: Context) -> Router {
    app
}

/// No private book-context receipt is attached after an ordinary reader GET.
pub(crate) fn book_response<T: TryInto<i64>>(
    response: Response,
    _book_id: T,
    _kind: i16,
) -> Response {
    response
}

#[derive(Clone, Copy)]
pub(crate) struct Snapshot {
    pub active: bool,
    pub offered: u64,
    pub dropped: u64,
    pub scope_timeouts: u64,
    pub saved: u64,
    pub skipped: u64,
    pub failures: u64,
}

/// These are process queue counters, not visitor or reading measurements.
pub(crate) fn snapshot() -> Snapshot {
    Snapshot {
        active: false,
        offered: 0,
        dropped: 0,
        scope_timeouts: 0,
        saved: 0,
        skipped: 0,
        failures: 0,
    }
}
